package app.agentero.mobile

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Process
import android.widget.Toast
import androidx.core.content.IntentCompat

/** Intent metadata is routing evidence only; the owned importer validates PDF bytes. */
internal object ExternalPdfIntentPolicy {
    /** Share extras are not Intent.data. Never reinterpret mismatching sources. */
    fun selectedUri(action: String?, data: String?, stream: String?, hasStream: Boolean): String? = when (action) {
        "android.intent.action.VIEW" -> data.takeUnless { hasStream }
        "android.intent.action.SEND" -> stream.takeIf { hasStream && (data == null || data == stream) }
        else -> null
    }

    fun accepts(
        action: String?,
        mime: String?,
        uri: String?,
        decodedAuthority: String?,
        clipUris: List<String?>,
        hasText: Boolean,
        hasStream: Boolean,
        hasReadFlag: Boolean,
        hasReadPermission: Boolean,
    ): Boolean = when (action) {
        "android.intent.action.VIEW" -> !hasStream
        "android.intent.action.SEND" -> hasStream
        else -> false
    } && mime == "application/pdf" &&
        uri != null && NativeImportFilePolicy.validUri(uri) &&
        !decodedAuthority.isNullOrBlank() && '@' !in decodedAuthority &&
        clipUris.size <= 1 && clipUris.all { it == uri } &&
        !hasText && hasReadFlag && hasReadPermission
}

/** One native-memory URI, never persisted or delivered to a renderer. First wins. */
internal class ExternalPdfMailbox {
    enum class Admission { ACCEPTED, BUSY, IGNORED }
    private var pending: String? = null
    private var initialRead = false

    @Synchronized fun captureInitial(read: () -> String?): Admission {
        if (initialRead) return Admission.IGNORED
        initialRead = true
        return offer(read())
    }

    @Synchronized fun offer(uri: String?): Admission {
        // A warm intent can arrive before load; never replay an older startup intent.
        initialRead = true
        if (uri == null) return Admission.IGNORED
        if (pending != null) return Admission.BUSY
        pending = uri
        return Admission.ACCEPTED
    }

    @Synchronized fun take(): String? = pending.also { pending = null }
    @Synchronized fun clear() { pending = null } // Do not replay the cold intent after disposal.
}

internal class ExternalPdfIntents(context: Context, private val pending: () -> Unit) {
    private val context = context.applicationContext
    private val mailbox = ExternalPdfMailbox()

    fun initial(intent: Intent?) = announce(mailbox.captureInitial { candidate(intent) })
    fun receive(intent: Intent) = announce(mailbox.offer(candidate(intent)))
    fun take(): String? = mailbox.take()
    fun clear() = mailbox.clear()

    private fun announce(admission: ExternalPdfMailbox.Admission) {
        when (admission) {
            ExternalPdfMailbox.Admission.ACCEPTED -> try { pending() } catch (_: Exception) { /* Retain for Rust's next take. */ }
            ExternalPdfMailbox.Admission.BUSY -> Toast.makeText(context, R.string.agentero_external_pdf_busy, Toast.LENGTH_LONG).show()
            ExternalPdfMailbox.Admission.IGNORED -> Unit
        }
    }

    private fun candidate(intent: Intent?): String? = try {
        if (intent == null || intent.action !in listOf(Intent.ACTION_VIEW, Intent.ACTION_SEND) || intent.type != "application/pdf") null
        else {
            val hasStream = intent.hasExtra(Intent.EXTRA_STREAM)
            // IntentCompat avoids Android 13's typed-Parcelable platform bug.
            val stream = if (intent.action == Intent.ACTION_SEND && hasStream) {
                IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)?.toString()
            } else null
            val uri = ExternalPdfIntentPolicy.selectedUri(intent.action, intent.data?.toString(), stream, hasStream)
            val selected = uri?.takeIf { NativeImportFilePolicy.validUri(it) }?.let { Uri.parse(it) }
            val clip = intent.clipData
            if (selected == null || clip != null && clip.itemCount > 1) null
            else {
                val item = if (clip != null && clip.itemCount == 1) clip.getItemAt(0) else null
                val hasText = intent.hasExtra(Intent.EXTRA_TEXT) || intent.hasExtra(Intent.EXTRA_HTML_TEXT) ||
                    item?.text != null || item?.htmlText != null || item?.intent != null
                val hasReadFlag = intent.flags and Intent.FLAG_GRANT_READ_URI_PERMISSION != 0
                if (ExternalPdfIntentPolicy.accepts(
                        intent.action, intent.type, uri, selected.authority,
                        if (item != null) listOf(item.uri?.toString()) else emptyList(),
                        hasText, hasStream, hasReadFlag,
                        hasReadFlag && context.checkUriPermission(
                            selected, Process.myPid(), Process.myUid(), Intent.FLAG_GRANT_READ_URI_PERMISSION,
                        ) == PackageManager.PERMISSION_GRANTED,
                    )) uri else null
            }
        }
    } catch (_: Exception) {
        null // Malformed external parcels and denied grants never reach the opener or logs.
    }
}
