package app.agentero.mobile

import android.app.Activity
import android.hardware.input.InputManager
import android.os.Handler
import android.os.Looper
import android.view.InputDevice
import android.view.KeyEvent
import android.webkit.WebView
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONArray
import org.json.JSONObject
import java.lang.ref.WeakReference

@InvokeArg
class StylusTargetArgs {
    var generation: String? = null
    var windowLabel: String? = null
    var targetToken: String? = null
    var active: Boolean? = null
}

/** OEM adapters must declare only public, implemented probes and dispose observers. */
internal interface StylusAdapter {
    val id: String
    fun probe(): JSObject
    fun presence(): JSObject
    fun start(changed: () -> Unit)
    fun dispose()
}

internal class GenericInputDeviceStylusAdapter(activity: Activity) : StylusAdapter, InputManager.InputDeviceListener {
    override val id = "android-input-device"
    private val input = requireNotNull(activity.getSystemService(InputManager::class.java))
    private var changed: (() -> Unit)? = null
    override fun probe() = JSObject().apply {
        put("id", id); put("source", "nativeInputDevice"); put("actions", JSONArray())
        put("doubleTap", false); put("squeeze", false); put("barrelButton", true); put("invertedPen", true)
        // InputDevice sources describe registered digitizers, not physical pen attachment.
        put("hardwarePresence", false)
    }
    override fun presence() = JSObject().apply {
        put("available", input.inputDeviceIds.any { input.getInputDevice(it)?.supportsSource(InputDevice.SOURCE_STYLUS) == true })
        put("proximity", JSONObject.NULL); put("source", "nativeInputDevice"); put("adapterId", id)
    }
    override fun start(changed: () -> Unit) {
        if (this.changed == null) input.registerInputDeviceListener(this, Handler(Looper.getMainLooper()))
        this.changed = changed
    }
    override fun dispose() { input.unregisterInputDeviceListener(this); changed = null }
    override fun onInputDeviceAdded(deviceId: Int) { changed?.invoke() }
    override fun onInputDeviceChanged(deviceId: Int) { changed?.invoke() }
    override fun onInputDeviceRemoved(deviceId: Int) { changed?.invoke() }
}

internal class StylusAdapterRegistry(activity: Activity, plugin: Plugin) {
    private val owner = WeakReference(activity)
    /**
     * Public Android 14+ barrel keycodes plus the keycodes emitted by Lenovo
     * Pen firmware and older Wacom-style pens. Mapping is only active while an
     * admitted PDF target is focused, so unknown keyboard keys are unaffected.
     */
    private val keyShortcuts = mapOf(
        KeyEvent.KEYCODE_STYLUS_BUTTON_PRIMARY to PencilShortcut.ToggleEraser,
        KeyEvent.KEYCODE_STYLUS_BUTTON_SECONDARY to PencilShortcut.PreviousTool,
        KeyEvent.KEYCODE_STYLUS_BUTTON_TERTIARY to PencilShortcut.ShowColorPalette,
        289 to PencilShortcut.ToggleEraser, // pre-Android-14 barrel primary
        290 to PencilShortcut.PreviousTool, // pre-Android-14 barrel secondary
        602 to PencilShortcut.ToggleEraser, // Lenovo framework PEN_PRESS_CLICK
        603 to PencilShortcut.PreviousTool,
        604 to PencilShortcut.ShowColorPalette,
    )
    private val plugin = WeakReference(plugin)
    private val adapters: List<StylusAdapter> = listOf(
        GenericInputDeviceStylusAdapter(activity),
        OplusPencilShortcutAdapter(activity) { shortcut -> emitShortcut(shortcut) },
    )
    private var generation = 0uL
    private var targetToken: String? = null
    private var windowLabel: String? = null
    private var started = false
    private var view = WeakReference<WebView>(null)

    fun attach(webView: WebView) { view = WeakReference(webView); start() }
    private fun focused(): Boolean = owner.get()?.hasWindowFocus() == true &&
        view.get()?.let { it.isAttachedToWindow && it.hasWindowFocus() && it.isShown } == true

    fun state(): JSObject = JSObject().apply {
        put("adapters", JSONArray().apply { adapters.forEach { put(it.probe()) } })
        put("presence", JSONArray().apply { adapters.forEach { put(it.presence()) } })
    }
    fun start() {
        if (started) return
        started = true
        adapters.forEach { it.start { emitPresence() } }
    }
    fun setTarget(args: StylusTargetArgs): JSObject {
        val raw = requireNotNull(args.generation)
        val next = raw.toULongOrNull()?.takeIf { it > 0uL && it.toString() == raw }
            ?: throw IllegalArgumentException("Invalid stylus generation")
        if (next <= generation) return state()
        val token = requireNotNull(args.targetToken)
        val label = requireNotNull(args.windowLabel)
        val active = requireNotNull(args.active)
        require(token.length == 36 && label.length in 1..128)
        generation = next
        if (active) {
            require(focused()) { "Stylus target window is inactive" }
            targetToken = token; windowLabel = label; start(); emitPresence()
        } else if (targetToken == token && windowLabel == label) {
            targetToken = null; windowLabel = null
        }
        return state()
    }
    /**
     * OEM body shortcuts carry no pen coordinates, so they may only change the
     * active tool. Rust re-checks the window and token before the WebView sees it.
     */
    private fun emitShortcut(shortcut: PencilShortcut, adapterId: String = OplusPencilShortcutAdapter.ID): Boolean {
        val token = targetToken ?: return false
        val label = windowLabel ?: return false
        val activity = owner.get() ?: return false
        if (activity.isDestroyed || activity.isFinishing || !focused()) return false
        val plugin = plugin.get() ?: return false
        plugin.trigger("stylus-event", JSObject().apply {
            put("kind", "action"); put("windowLabel", label)
            put("event", JSObject().apply {
                put("action", shortcut.wire)
                put("source", "nativeInputDevice")
                put("adapterId", adapterId)
                put("targetToken", token)
            })
        })
        return true
    }

    /** Returns true only when a focused PDF target consumed the key. */
    fun handleKey(keyCode: Int): Boolean {
        val shortcut = keyShortcuts[keyCode] ?: return false
        return emitShortcut(shortcut, "android-stylus-key")
    }

    private fun emitPresence() {
        val token = targetToken ?: return
        val label = windowLabel ?: return
        val activity = owner.get() ?: return
        if (activity.isDestroyed || activity.isFinishing || !focused()) return
        adapters.forEach { adapter ->
            plugin.get()?.trigger("stylus-event", JSObject().apply {
                put("kind", "presence"); put("windowLabel", label); put("targetToken", token)
                put("presence", adapter.presence())
            })
        }
    }
    fun dispose() {
        targetToken = null; windowLabel = null
        adapters.forEach { it.dispose() }; started = false
    }
}
