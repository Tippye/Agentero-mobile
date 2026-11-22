package app.agentero.mobile

import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/** Only raises process priority for the existing Rust work; it starts no engine. */
abstract class AndroidBackgroundService internal constructor(private val lane: BackgroundLane) : Service() {
    private var foregroundStarted = false
    private val controller get() = AndroidBackgroundController.get(this)

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val token = intent?.getLongExtra(AndroidBackgroundController.START_TOKEN, -1L) ?: -1L
        if (!controller.permits(lane, token)) {
            if (!foregroundStarted) stopSelf(startId)
            return START_NOT_STICKY
        }
        try {
            val notification = AndroidBackgroundNotifications.notification(this, lane)
            if (Build.VERSION.SDK_INT >= 34) {
                val type = if (lane == BackgroundLane.AGENT) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
                startForeground(AndroidBackgroundNotifications.id(lane), notification, type)
            } else {
                // API 33 has no specialUse type; use the normal foreground-service API.
                startForeground(AndroidBackgroundNotifications.id(lane), notification)
            }
            foregroundStarted = true
            controller.started(lane, token, this)
        } catch (error: Exception) {
            releaseForeground()
            controller.failed(lane, token, error)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    fun updateNotification() {
        if (foregroundStarted) AndroidBackgroundNotifications.manager(this).notify(
            AndroidBackgroundNotifications.id(lane), AndroidBackgroundNotifications.notification(this, lane),
        )
    }

    fun finish() {
        releaseForeground()
        stopSelf()
    }

    private fun releaseForeground() {
        if (foregroundStarted) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            foregroundStarted = false
        }
        controller.stopped(lane, this)
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        controller.taskRemoved()
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        releaseForeground()
        super.onDestroy()
    }
}
