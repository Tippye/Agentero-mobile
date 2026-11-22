package app.agentero.mobile

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings

internal object AndroidBackgroundSettings {
    fun intents(context: Context, target: String): List<Intent> {
        val app = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
        return when (target) {
            "battery" -> listOf(
                Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.fromParts("package", context.packageName, null)),
                Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS), app,
            )
            "notifications" -> listOf(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName), app)
            "app" -> listOf(app)
            else -> throw IllegalArgumentException("Unknown background settings target")
        }
    }
}
