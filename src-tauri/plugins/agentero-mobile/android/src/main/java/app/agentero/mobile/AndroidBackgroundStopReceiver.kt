package app.agentero.mobile

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** An immutable explicit PendingIntent reaches this non-exported receiver. */
class AndroidBackgroundStopReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == AndroidBackgroundNotifications.STOP_ACTION) {
            AndroidBackgroundController.get(context).stopByUser()
        }
    }
}
