package app.agentero.mobile

import android.app.Activity
import android.app.Application
import android.os.Bundle
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner

/** Visibility includes started-but-not-resumed split-screen activities. */
internal class AndroidForegroundTracker(
    application: Application,
    private val changed: () -> Unit,
) : Application.ActivityLifecycleCallbacks {
    private val visibility = StartedActivityVisibility()
    val foreground: Boolean get() = visibility.foreground

    init {
        application.registerActivityLifecycleCallbacks(this)
    }

    fun attach(activity: Activity) {
        if ((activity as? LifecycleOwner)?.lifecycle?.currentState?.isAtLeast(Lifecycle.State.STARTED) == true) {
            update(activity, true)
        }
    }

    private fun update(activity: Activity, visible: Boolean) {
        if (visibility.update(activity, visible)) changed()
    }

    override fun onActivityStarted(activity: Activity) = update(activity, true)
    override fun onActivityStopped(activity: Activity) = update(activity, false)
    override fun onActivityDestroyed(activity: Activity) = update(activity, false)
    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
    override fun onActivityResumed(activity: Activity) = Unit
    override fun onActivityPaused(activity: Activity) = Unit
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
}
