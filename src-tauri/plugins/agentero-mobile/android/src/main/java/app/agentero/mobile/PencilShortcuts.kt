package app.agentero.mobile

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.provider.Settings
import androidx.core.content.ContextCompat
import app.tauri.plugin.JSObject
import org.json.JSONObject
import java.lang.ref.WeakReference

/**
 * Pencil body shortcut exposed by OPPO/OnePlus tablets. Only the vendor-documented
 * broadcast and its public settings value are used; no OEM SDK class is referenced
 * here, so an app without the separate pen service authorization still behaves
 * exactly as before.
 */
internal enum class PencilShortcut(val wire: String) {
    ToggleEraser("toggleEraser"),
    PreviousTool("previousTool"),
    ShowColorPalette("showColorPalette"),
}

internal class OplusPencilShortcutAdapter(
    activity: Activity,
    private val onShortcut: (PencilShortcut) -> Unit,
) : StylusAdapter {
    private val context = WeakReference(activity.applicationContext)
    private var registered = false
    /** Evidence that this ROM actually delivers the vendor shortcut, not a guess. */
    private var seen = false
    private var changed: (() -> Unit)? = null

    override val id = ID

    // Documented values of Settings.Global[ipe_pencil_double_click]; 0 means the
    // user turned the pen shortcut off in system settings.
    private val shortcuts = mapOf(
        1 to PencilShortcut.ToggleEraser,
        2 to PencilShortcut.PreviousTool,
        3 to PencilShortcut.ShowColorPalette,
    )

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            if (!registered || intent?.action != ACTION) return
            val shortcut = shortcuts[setting()] ?: return
            if (!seen) {
                seen = true
                changed?.invoke()
            }
            onShortcut(shortcut)
        }
    }

    private fun setting(): Int {
        val context = context.get() ?: return 0
        return Settings.Global.getInt(context.contentResolver, SETTING, 0)
    }

    /**
     * Declared interface, not connected hardware: the vendor pen service may be
     * absent on a rebranded ROM even when the tablet accepts the same pencil.
     */
    override fun probe(): JSObject = JSObject().apply {
        put("id", id)
        put("source", "nativeInputDevice")
        put(
            "actions",
            org.json.JSONArray().apply {
                if (registered) for (shortcut in shortcuts.values) put(shortcut.wire)
            },
        )
        put("doubleTap", registered)
        put("squeeze", false)
        put("barrelButton", false)
        put("invertedPen", false)
        put("hardwarePresence", false)
    }

    override fun presence(): JSObject = JSObject().apply {
        // Only an actually delivered shortcut proves this ROM routes the event.
        put("available", registered && seen)
        put("proximity", JSONObject.NULL)
        put("source", "nativeInputDevice")
        put("adapterId", id)
    }

    override fun start(changed: () -> Unit) {
        this.changed = changed
        if (registered) return
        val context = context.get() ?: return
        registered = try {
            // Registered exported because the sender is the vendor pen service.
            // The manifest requests the documented sender permission; it is not
            // passed here because OEM ROMs disagree on which side enforces it.
            // A spoofed shortcut can only switch the active tool, and only while a
            // PDF ink target is admitted by Rust, so impact stays bounded.
            ContextCompat.registerReceiver(
                context,
                receiver,
                IntentFilter(ACTION),
                ContextCompat.RECEIVER_EXPORTED,
            )
            true
        } catch (_: SecurityException) {
            // ROM lacks the vendor permission or changed its protection level.
            false
        } catch (_: IllegalArgumentException) {
            false
        }
        if (registered) changed()
    }

    override fun dispose() {
        changed = null
        seen = false
        if (!registered) return
        registered = false
        context.get()?.let { context ->
            try {
                context.unregisterReceiver(receiver)
            } catch (_: IllegalArgumentException) {
                // Already unregistered by the system.
            }
        }
    }

    companion object {
        const val ID = "oplus-pencil-shortcut"
        const val ACTION = "com.oplus.ipemanager.action.PENCIL_DOUBLE_CLICK"
        const val SETTING = "ipe_pencil_double_click"
    }
}