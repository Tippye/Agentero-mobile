package app.agentero.mobile

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager

internal object AndroidBackgroundNotifications {
    const val AGENT_CHANNEL = "agentero_background_agent"
    const val SYNC_CHANNEL = "agentero_background_sync"
    const val AGENT_ID = 46_301
    const val SYNC_ID = 46_302
    const val STOP_ACTION = "app.agentero.mobile.STOP_BACKGROUND"

    fun manager(context: Context): NotificationManager = context.getSystemService(NotificationManager::class.java)

    fun ensureChannels(context: Context) {
        val manager = manager(context)
        listOf(
            AGENT_CHANNEL to R.string.agentero_bg_agent_channel,
            SYNC_CHANNEL to R.string.agentero_bg_sync_channel,
        ).forEach { (id, name) ->
            manager.createNotificationChannel(NotificationChannel(id, context.getString(name), NotificationManager.IMPORTANCE_LOW).apply {
                lockscreenVisibility = Notification.VISIBILITY_PRIVATE
                setShowBadge(false)
            })
        }
    }

    fun granted(context: Context): Boolean =
        context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    fun enabled(context: Context, lane: BackgroundLane? = null): Boolean {
        val manager = manager(context)
        if (!manager.areNotificationsEnabled()) return false
        val channels = when (lane) {
            BackgroundLane.AGENT -> listOf(AGENT_CHANNEL)
            BackgroundLane.SYNC -> listOf(SYNC_CHANNEL)
            null -> listOf(AGENT_CHANNEL, SYNC_CHANNEL)
        }
        return channels.all { manager.getNotificationChannel(it)?.importance?.let { value -> value != NotificationManager.IMPORTANCE_NONE } == true }
    }

    fun id(lane: BackgroundLane): Int = if (lane == BackgroundLane.AGENT) AGENT_ID else SYNC_ID

    fun notification(context: Context, lane: BackgroundLane): Notification {
        val channel = if (lane == BackgroundLane.AGENT) AGENT_CHANNEL else SYNC_CHANNEL
        val title = if (lane == BackgroundLane.AGENT) R.string.agentero_bg_agent_title else R.string.agentero_bg_sync_title
        val stop = PendingIntent.getBroadcast(
            context,
            0,
            Intent(context, AndroidBackgroundStopReceiver::class.java).setAction(STOP_ACTION),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val public = Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.agentero_bg_notification)
            .setContentTitle(context.getString(R.string.agentero_bg_public_title))
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .build()
        val builder = Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.agentero_bg_notification)
            .setContentTitle(context.getString(title))
            .setContentText(context.getString(R.string.agentero_bg_running))
            .setCategory(Notification.CATEGORY_SERVICE)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public)
            .setOnlyAlertOnce(true)
            .setOngoing(true)
            .setShowWhen(false)
            .setLocalOnly(true)
            .setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
            .addAction(Notification.Action.Builder(null, context.getString(R.string.agentero_bg_stop), stop).build())
        context.packageManager.getLaunchIntentForPackage(context.packageName)?.let { intent ->
            builder.setContentIntent(PendingIntent.getActivity(context, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE))
        }
        return builder.build()
    }
}
