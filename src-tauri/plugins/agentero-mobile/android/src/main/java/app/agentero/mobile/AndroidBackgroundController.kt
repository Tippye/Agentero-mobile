package app.agentero.mobile

import android.app.Activity
import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.app.NotificationManager
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import app.tauri.plugin.JSObject
import java.util.Collections
import java.util.EnumMap
import java.util.WeakHashMap

internal interface AndroidBackgroundEvents {
    fun backgroundStateChanged()
    fun backgroundExpired()
}

/** Application-scoped coordinator; it owns no Activity, task details, engine or wake lock. */
internal class AndroidBackgroundController private constructor(private val application: Application) {
    private val preferences = application.getSharedPreferences("agentero_background", Context.MODE_PRIVATE)
    private val handler = Handler(Looper.getMainLooper())
    private val listeners = Collections.newSetFromMap(WeakHashMap<AndroidBackgroundEvents, Boolean>())
    private val active = EnumMap<BackgroundLane, AndroidBackgroundService>(BackgroundLane::class.java)
    private val pending = EnumMap<BackgroundLane, Long>(BackgroundLane::class.java)
    private val waiters = mutableListOf<ApplyWaiter>()
    private val generations = BackgroundGenerationFence()
    private var sequence = 0L
    private var desired = BackgroundCounts()
    private var syncExpired = false
    private var enabled = preferences.getBoolean("enabled", false)
    private val visibility = AndroidForegroundTracker(application) { visibilityChanged() }

    private data class ApplyWaiter(val counts: BackgroundCounts, val done: (Result<Unit>) -> Unit)

    init {
        AndroidBackgroundNotifications.ensureChannels(application)
        application.registerReceiver(object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) = refresh()
        }, IntentFilter().apply {
            addAction(NotificationManager.ACTION_APP_BLOCK_STATE_CHANGED)
            addAction(NotificationManager.ACTION_NOTIFICATION_CHANNEL_BLOCK_STATE_CHANGED)
            addAction(NotificationManager.ACTION_NOTIFICATION_CHANNEL_GROUP_BLOCK_STATE_CHANGED)
        }, Context.RECEIVER_NOT_EXPORTED)
    }

    fun attach(activity: Activity, events: AndroidBackgroundEvents) {
        checkMainThread()
        listeners.add(events)
        visibility.attach(activity)
    }

    fun detach(events: AndroidBackgroundEvents) {
        checkMainThread()
        listeners.remove(events)
    }

    fun requireForeground() {
        checkMainThread()
        check(visibility.foreground) { "Open Agentero before changing background permissions" }
    }

    fun state(permission: String): JSObject {
        checkMainThread()
        val battery = runCatching {
            application.getSystemService(PowerManager::class.java)?.isIgnoringBatteryOptimizations(application.packageName)
        }.getOrNull()
        return JSObject().apply {
            put("platform", "android")
            put("persistentConnection", true)
            put("finiteTasks", false)
            put("backgroundTransfer", false)
            put("enabled", enabled)
            put("foreground", visibility.foreground)
            put("notificationPermission", permission)
            put("notificationsEnabled", AndroidBackgroundNotifications.enabled(application))
            put("batteryOptimizationIgnored", battery ?: org.json.JSONObject.NULL)
            put("agentActive", active.containsKey(BackgroundLane.AGENT))
            put("syncActive", active.containsKey(BackgroundLane.SYNC))
            put("finiteTaskActive", false)
            put("finiteTaskExpired", false)
        }
    }

    fun configure(value: Boolean) {
        checkMainThread()
        if (value) {
            check(AndroidBackgroundPolicy.canEnable(gate())) {
                "Allow notifications and both background channels while Agentero is visible"
            }
        } else {
            // Opt-out takes effect even if persisting the preference fails.
            enabled = false
            stopAll()
        }
        check(preferences.edit().putBoolean("enabled", value).commit()) {
            "Could not save the background setting"
        }
        enabled = value
        changed()
    }

    /** Completion waits for real startForeground acknowledgement, never just a launch request. */
    fun apply(generation: String, counts: BackgroundCounts, done: (Result<Unit>) -> Unit) {
        checkMainThread()
        if (!generations.accept(generation)) {
            done(Result.success(Unit))
            return
        }
        desired = counts
        if (!counts.desired(BackgroundLane.SYNC)) syncExpired = false
        waiters.add(ApplyWaiter(counts, done))
        runCatching {
            BackgroundLane.entries.forEach { lane ->
                when (AndroidBackgroundPolicy.action(
                    counts.desired(lane), gate(lane), observed(lane),
                    expired = lane == BackgroundLane.SYNC && syncExpired,
                )) {
                    BackgroundAction.START -> start(lane)
                    BackgroundAction.STOP -> stop(lane)
                    BackgroundAction.KEEP -> active[lane]?.updateNotification()
                    BackgroundAction.IDLE -> Unit
                }
            }
        }.onFailure { error -> completeWaiters(error) }
        completeWaiters()
    }

    fun permits(lane: BackgroundLane, token: Long): Boolean {
        checkMainThread()
        // The launch was authorized while visible; an Activity may stop before delivery.
        return pending[lane] == token && desired.desired(lane) && enabled &&
            AndroidBackgroundNotifications.granted(application) &&
            AndroidBackgroundNotifications.enabled(application, lane) &&
            !(lane == BackgroundLane.SYNC && syncExpired)
    }

    fun started(lane: BackgroundLane, token: Long, service: AndroidBackgroundService) {
        checkMainThread()
        if (!permits(lane, token)) {
            failed(lane, token, IllegalStateException("Background start prerequisites changed"))
            service.finish()
            return
        }
        pending.remove(lane)
        active[lane] = service
        completeWaiters()
        changed()
    }

    fun failed(lane: BackgroundLane, token: Long, error: Throwable) {
        checkMainThread()
        if (pending[lane] != token) return
        pending.remove(lane)
        completeWaiters(error)
        changed()
    }

    fun stopped(lane: BackgroundLane, service: AndroidBackgroundService) {
        checkMainThread()
        if (active[lane] === service) {
            active.remove(lane)
            changed()
        }
        // System destruction never starts a replacement service.
    }

    fun timeout() {
        checkMainThread()
        syncExpired = true
        stop(BackgroundLane.SYNC)
        completeWaiters(IllegalStateException("Android data sync reached its foreground service timeout"))
        notifyListeners { it.backgroundExpired() }
        changed()
    }

    fun stopByUser() {
        checkMainThread()
        enabled = false
        stopAll()
        // No remote task cancellation, lease labels, credentials or prompts are stored here.
        if (!preferences.edit().putBoolean("enabled", false).commit()) {
            application.deleteSharedPreferences("agentero_background")
        }
        changed()
    }

    fun taskRemoved() {
        checkMainThread()
        stopAll()
        changed()
    }

    fun refresh() {
        checkMainThread()
        // Returning from system settings can revoke a running service's prerequisites.
        BackgroundLane.entries.forEach { lane ->
            if (!enabled || !AndroidBackgroundNotifications.granted(application) ||
                !AndroidBackgroundNotifications.enabled(application, lane)) stop(lane)
        }
        completeWaiters()
        changed()
    }

    private fun gate(lane: BackgroundLane? = null) = BackgroundGate(
        enabled, visibility.foreground,
        AndroidBackgroundNotifications.granted(application),
        AndroidBackgroundNotifications.enabled(application, lane),
    )

    private fun observed(lane: BackgroundLane) = BackgroundObserved(active.containsKey(lane), pending.containsKey(lane))

    private fun start(lane: BackgroundLane) {
        val token = ++sequence
        pending[lane] = token
        try {
            application.startForegroundService(intent(lane).putExtra(START_TOKEN, token))
        } catch (error: Exception) {
            pending.remove(lane)
            throw error
        }
        handler.postDelayed({
            if (pending[lane] == token) {
                stop(lane)
                completeWaiters(IllegalStateException("Android did not acknowledge the foreground service start"))
                changed()
            }
        }, START_TIMEOUT_MS)
    }

    private fun stop(lane: BackgroundLane) {
        pending.remove(lane)
        active[lane]?.finish()
        application.stopService(intent(lane))
    }

    private fun stopAll() {
        BackgroundLane.entries.forEach(::stop)
        completeWaiters(IllegalStateException("Background activity stopped"))
    }

    private fun completeWaiters(error: Throwable? = null) {
        val ready = waiters.filter { waiter ->
            error != null || waiter.counts != desired ||
                BackgroundLane.entries.none { waiter.counts.desired(it) && pending.containsKey(it) }
        }
        waiters.removeAll(ready.toSet())
        ready.forEach { waiter ->
            val failure = error ?: if (waiter.counts != desired) IllegalStateException("Background request superseded") else null
            waiter.done(if (failure == null) Result.success(Unit) else Result.failure(failure))
        }
    }

    private fun visibilityChanged() {
        refresh()
    }

    private fun changed() {
        notifyListeners { it.backgroundStateChanged() }
    }

    private fun notifyListeners(deliver: (AndroidBackgroundEvents) -> Unit) {
        listeners.toList().forEach { listener ->
            // A destroyed window must not break service cleanup or other live listeners.
            runCatching { deliver(listener) }.onFailure { listeners.remove(listener) }
        }
    }

    private fun intent(lane: BackgroundLane) = Intent(application, when (lane) {
        BackgroundLane.AGENT -> AndroidAgentConnectionService::class.java
        BackgroundLane.SYNC -> AndroidDataSyncService::class.java
    })

    companion object {
        const val START_TOKEN = "agentero_background_start_token"
        private const val START_TIMEOUT_MS = 4_000L
        @Volatile private var instance: AndroidBackgroundController? = null

        fun get(context: Context): AndroidBackgroundController = instance ?: synchronized(this) {
            instance ?: AndroidBackgroundController(context.applicationContext as Application).also { instance = it }
        }

        private fun checkMainThread() {
            check(Looper.myLooper() == Looper.getMainLooper()) { "Background lifecycle must run on the main thread" }
        }
    }
}
