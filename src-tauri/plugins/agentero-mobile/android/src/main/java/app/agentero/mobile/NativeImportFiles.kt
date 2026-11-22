package app.agentero.mobile

import android.content.Context
import android.content.pm.PackageManager
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.os.Process
import android.provider.OpenableColumns
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.net.URI
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

@InvokeArg
internal class OpenImportFileArgs {
    lateinit var uri: String
}

/** Validation and descriptor ownership are independent of provider/device behavior. */
internal object NativeImportFilePolicy {
    fun validUri(value: String): Boolean = value.length in 1..32768 && try {
        val uri = URI(value)
        uri.scheme == "content" && !uri.authority.isNullOrBlank() &&
            '@' !in uri.authority && !uri.isOpaque
    } catch (_: Exception) {
        false
    }

    /** A picker result cannot authorize reads through our own/shared-UID providers. */
    fun <Opened> openExternal(
        authority: String,
        ownUid: Int,
        providerUid: (String) -> Int?,
        open: () -> Opened,
    ): Opened {
        require(authority.isNotBlank() && '@' !in authority) { "Import provider unavailable" }
        val uid = providerUid(authority)
        require(ownUid >= 0 && uid != null && uid >= 0 && uid != ownUid) { "Import provider unavailable" }
        return open()
    }

    /** Optional provider metadata is a name only, never a path or read authority. */
    fun displayName(read: () -> String?): String? = try {
        val raw = read()
        if (raw == null || !wellFormedUtf16(raw) || raw.any { it.isISOControl() || Character.getType(it) == Character.FORMAT.toInt() }) {
            null
        } else {
            val name = raw.trim()
            name.takeIf {
                it.isNotBlank() && it != "." && it != ".." &&
                    '/' !in it && '\\' !in it && it.codePointCount(0, it.length) <= 512
            }
        }
    } catch (_: Exception) {
        null // Metadata failure must not reject a readable selected descriptor.
    }

    private fun wellFormedUtf16(value: String): Boolean {
        var index = 0
        while (index < value.length) {
            val character = value[index]
            if (Character.isHighSurrogate(character)) {
                if (index + 1 >= value.length || !Character.isLowSurrogate(value[index + 1])) return false
                index += 2
            } else {
                if (Character.isLowSurrogate(character)) return false
                index += 1
            }
        }
        return true
    }

    fun <Response> transfer(
        descriptor: () -> Int,
        detach: () -> Int,
        closeWrapper: () -> Unit,
        closeDetached: (Int) -> Unit,
        prepare: (Int) -> Response,
        deliver: (Response) -> Unit,
    ) {
        var detached = -1
        var wrapperCloseAttempted = false
        var deliveryAttempted = false
        try {
            val fd = descriptor()
            check(fd >= 0) { "Import descriptor unavailable" }
            // Prepare the response while the ParcelFileDescriptor still owns the fd.
            val response = prepare(fd)
            detached = detach()
            check(detached == fd) { "Import descriptor unavailable" }
            wrapperCloseAttempted = true
            closeWrapper() // A detached wrapper no longer owns the raw descriptor.
            deliveryAttempted = true
            deliver(response)
        } finally {
            try {
                if (!wrapperCloseAttempted) closeWrapper()
            } finally {
                // Once JNI response delivery has been attempted, Rust may already own
                // (and even have closed/reused) this number. There is no delivery ack.
                if (!deliveryAttempted && detached >= 0) closeDetached(detached)
            }
        }
    }
}

/** Rust-only selected-URI adapter. The renderer never supplies a URI to this command. */
internal class NativeImportFiles(context: Context) {
    private val resolver = context.applicationContext.contentResolver
    private val packages = context.applicationContext.packageManager

    fun open(invoke: Invoke) {
        val uri = try {
            invoke.parseArgs(OpenImportFileArgs::class.java).uri.also {
                require(NativeImportFilePolicy.validUri(it))
            }
        } catch (_: Exception) {
            reject(invoke)
            return
        }
        open(invoke, uri)
    }

    /** The URI was retained by native intent intake, never supplied by JavaScript. */
    fun open(invoke: Invoke, uri: String) {
        if (!NativeImportFilePolicy.validUri(uri)) {
            reject(invoke)
            return
        }
        try {
            worker.execute {
                try {
                    // Unlike openAssetFileDescriptor, this rejects a provider's partial
                    // asset instead of discarding its offset/length. Read-only pipes work.
                    val selected = Uri.parse(uri)
                    val parcel = NativeImportFilePolicy.openExternal(
                        authority = requireNotNull(selected.authority),
                        ownUid = Process.myUid(),
                        providerUid = { authority ->
                            packages.resolveContentProvider(authority, PackageManager.ComponentInfoFlags.of(0))?.applicationInfo?.uid
                        },
                        open = { requireNotNull(resolver.openFileDescriptor(selected, "r")) },
                    )
                    NativeImportFilePolicy.transfer(
                        descriptor = { parcel.fd },
                        detach = { parcel.detachFd() },
                        closeWrapper = { parcel.close() },
                        closeDetached = { ParcelFileDescriptor.adoptFd(it).close() },
                        prepare = { fd ->
                            val name = NativeImportFilePolicy.displayName {
                                resolver.query(selected, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                                    val column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                                    if (column >= 0 && cursor.moveToFirst() && !cursor.isNull(column)) cursor.getString(column) else null
                                }
                            }
                            JSObject().put("fd", fd).apply {
                                if (name != null) put("displayName", name)
                            }
                        },
                        deliver = { invoke.resolve(it) },
                    )
                } catch (_: Exception) {
                    reject(invoke)
                }
            }
        } catch (_: Exception) {
            reject(invoke)
        }
    }

    companion object {
        // Bounded admission prevents a slow cloud provider from accumulating threads.
        private val worker = ThreadPoolExecutor(
            1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue<Runnable>(1),
            { task -> Thread(task, "AgenteroImportFile").apply { isDaemon = true } },
            ThreadPoolExecutor.AbortPolicy(),
        )

        private fun reject(invoke: Invoke) {
            try {
                invoke.reject("Selected import file is unavailable", "IMPORT_FILE_UNAVAILABLE")
            } catch (_: Exception) {
                // Response delivery can already be gone. Never expose provider URI/errors
                // to PluginManager's reflection exception logger or retry an uncertain fd.
            }
        }
    }
}
