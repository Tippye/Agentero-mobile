package app.agentero.mobile

import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.lang.ref.WeakReference

/** The process coordinator cannot retain a Plugin's Activity through its event sink. */
internal class AndroidBackgroundEventRelay(plugin: Plugin) : AndroidBackgroundEvents {
    private val target = WeakReference(plugin)

    override fun backgroundStateChanged() {
        target.get()?.trigger("background-state", JSObject())
    }

    override fun backgroundExpired() {
        target.get()?.trigger("background-expired", JSObject().apply {
            put("kind", "sync")
            put("reason", "timeout")
        })
    }
}
