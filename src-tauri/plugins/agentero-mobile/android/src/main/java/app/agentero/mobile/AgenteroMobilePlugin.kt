package app.agentero.mobile

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.view.KeyEvent
import android.webkit.WebView
import androidx.appcompat.app.AppCompatActivity
import app.tauri.PermissionState
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/** Rust-only platform adapter; policy and work ownership stay outside the WebView. */
@TauriPlugin(permissions = [Permission(alias = "notifications", strings = [Manifest.permission.POST_NOTIFICATIONS])])
class AgenteroMobilePlugin(private val activity: Activity) : Plugin(activity) {
    private val controller by lazy { AndroidBackgroundController.get(activity) }
    private val events = AndroidBackgroundEventRelay(this)
    private val connectivityEvents = ConnectivityEventRelay(this)
    private val importFiles by lazy { NativeImportFiles(activity.applicationContext) }
    private val externalPdfs by lazy {
        ExternalPdfIntents(activity.applicationContext) { trigger("external-pdf-pending", JSObject()) }
    }
    private val stylus by lazy { StylusAdapterRegistry(activity, this) }

    override fun load(webView: WebView) {
        // Typesetting a translated PDF needs embedded font bytes; stage the
        // bundled fallback off the UI thread before the reader can ask for it.
        ReflowFontAssets.warm(activity.applicationContext)
        activity.runOnUiThread {
            controller.attach(activity, events); stylus.attach(webView)
            // Android 14+ turns barrel-button presses into KeyEvents and sends
            // them to the app first. Intercept only the stylus/vendor keycodes
            // we map; every other key falls through to the WebView unchanged.
            webView.setOnKeyListener { _, keyCode, event ->
                event.action == KeyEvent.ACTION_DOWN &&
                    event.repeatCount == 0 &&
                    stylus.handleKey(keyCode)
            }
            externalPdfs.initial(activity.intent)
        }
    }

    override fun onNewIntent(intent: Intent) {
        activity.runOnUiThread { externalPdfs.receive(intent) }
    }

    @Command
    fun readState(invoke: Invoke) = onUi(invoke) { invoke.resolve(state()) }

    @Command
    fun connectivityState(invoke: Invoke) = onUi(invoke) {
        invoke.resolve(ConnectivityMonitor.get(activity.applicationContext).state(connectivityEvents))
    }

    @Command
    fun openImportFile(invoke: Invoke) = importFiles.open(invoke)

    @Command
    fun ensureDictionary(invoke: Invoke) = DictionaryAssets.ensure(activity, invoke)

    @Command
    fun takeExternalPdf(invoke: Invoke) = onUi(invoke) {
        val uri = externalPdfs.take()
        if (uri == null) {
            invoke.resolve(JSObject().put("fd", org.json.JSONObject.NULL).put("displayName", org.json.JSONObject.NULL))
        } else {
            importFiles.open(invoke, uri)
        }
    }

    @Command
    fun configure(invoke: Invoke) = onUi(invoke) {
        val args = invoke.parseArgs(AndroidBackgroundConfigureArgs::class.java)
        controller.configure(requireNotNull(args.enabled) { "Missing enabled setting" })
        invoke.resolve(state())
    }

    @Command
    fun requestNotifications(invoke: Invoke) = onUi(invoke) {
        controller.requireForeground()
        if (AndroidBackgroundNotifications.granted(activity)) {
            invoke.resolve(state())
        } else {
            requestPermissionForAlias("notifications", invoke, "notificationsResult")
        }
    }

    @PermissionCallback
    fun notificationsResult(invoke: Invoke) = onUi(invoke) {
        controller.refresh()
        invoke.resolve(state())
    }

    @Command
    fun openSettings(invoke: Invoke) = onUi(invoke) {
        controller.requireForeground()
        val args = invoke.parseArgs(AndroidBackgroundSettingsArgs::class.java)
        var failure: RuntimeException? = null
        for (intent in AndroidBackgroundSettings.intents(activity, requireNotNull(args.target) { "Missing settings target" })) {
            try {
                startActivityForResult(invoke, intent, "settingsResult")
                return@onUi
            } catch (error: RuntimeException) {
                if (error !is ActivityNotFoundException && error !is SecurityException) throw error
                failure = error
            }
        }
        throw failure ?: IllegalStateException("Android settings are unavailable")
    }

    @ActivityCallback
    fun settingsResult(invoke: Invoke, result: androidx.activity.result.ActivityResult) = onUi(invoke) {
        controller.refresh()
        invoke.resolve(state())
    }

    @Command
    fun apply(invoke: Invoke) = onUi(invoke) {
        val args = invoke.parseArgs(AndroidBackgroundApplyArgs::class.java)
        controller.apply(requireNotNull(args.generation) { "Missing background generation" }, args.counts()) { result ->
            result.fold(
                onSuccess = { invoke.resolve(state()) },
                onFailure = { invoke.reject(it.message ?: "Android background activity failed", "BACKGROUND_APPLY_FAILED") },
            )
        }
    }

    @Command
    fun stylusState(invoke: Invoke) = onUi(invoke) { invoke.resolve(stylus.state()) }

    @Command
    fun stylusSetTarget(invoke: Invoke) = onUi(invoke) {
        invoke.resolve(stylus.setTarget(invoke.parseArgs(StylusTargetArgs::class.java)))
    }

    override fun onDestroy(activity: AppCompatActivity) {
        activity.runOnUiThread { externalPdfs.clear(); stylus.dispose() }
    }

    override fun onResume(activity: AppCompatActivity) {
        activity.runOnUiThread {
            controller.attach(activity, events)
            controller.refresh()
        }
    }

    // PluginManager retains this plugin and its Rust channels after Activity destruction.
    // Keep the process-lifetime weak relay for headless dataSync timeout delivery;
    // Application.ActivityLifecycleCallbacks independently removes destroyed visibility entries.

    private fun state(): JSObject {
        val permission = if (AndroidBackgroundNotifications.granted(activity)) "granted" else when (getPermissionState("notifications")) {
            PermissionState.PROMPT, null -> "prompt"
            else -> "denied"
        }
        return controller.state(permission)
    }

    private fun onUi(invoke: Invoke, action: () -> Unit) {
        activity.runOnUiThread {
            try {
                action()
            } catch (error: Exception) {
                invoke.reject(error.message ?: "Android background operation failed", "BACKGROUND_OPERATION_FAILED")
            }
        }
    }
}
