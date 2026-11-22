package app.agentero.mobile

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.Typeface
import android.net.Uri
import android.provider.Settings
import android.util.TypedValue
import android.view.ContextThemeWrapper
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.webkit.WebSettings
import android.webkit.WebView
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast

/** Native fallback before Wry's first application navigation; SDK lifecycle stays intact. */
class WebViewCompatibilityGuard(private val activity: Activity) {
    private var decision = WebViewCompatibilityPolicy.Decision(WebViewCompatibilityPolicy.Status.UNKNOWN)
    private var panel: View? = null

    fun onWebViewCreate(webView: WebView) {
        // The default UA describes the initialized engine, not an app override or an
        // OEM package's marketing version. Discard it after deriving the decision.
        decision = WebViewCompatibilityPolicy.evaluate { WebSettings.getDefaultUserAgent(activity) }
        if (decision.blocksContent) webView.settings.javaScriptEnabled = false
    }

    fun contentFor(view: View?): View? =
        if (decision.blocksContent && view is WebView) {
            panel ?: createPanel().also { panel = it }
        } else view

    /** Call after Activity/AppCompat has applied the new resource configuration. */
    fun onConfigurationChanged() {
        val previous = panel ?: return
        val parent = previous.parent as? ViewGroup
        if (parent == null) {
            panel = null
            return
        }
        if (!decision.blocksContent) return
        val index = parent.indexOfChild(previous)
        if (index < 0) return
        val params = previous.layoutParams
        val replacement = createPanel()
        panel = replacement
        parent.removeViewAt(index)
        parent.addView(replacement, index, params)
    }

    private fun createPanel(): View {
        // Reapply the manifest DayNight theme to updated resources. Cached Views
        // retain their old strings, SP/DP dimensions and resolved theme attributes.
        val context = ContextThemeWrapper(activity, activity.applicationInfo.theme)
        val content = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(24), dp(24), dp(24), dp(24))
        }
        content.addView(TextView(context).apply {
            setText(R.string.agentero_webview_guard_title)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 24f)
            setTypeface(typeface, Typeface.BOLD)
            isAccessibilityHeading = true
        }, row())
        content.addView(TextView(context).apply {
            text = context.getString(
                R.string.agentero_webview_guard_description,
                decision.major,
                WebViewCompatibilityPolicy.MINIMUM_CHROME_MAJOR,
            )
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 18f)
        }, row(dp(16)))
        content.addView(Button(context).apply {
            setText(R.string.agentero_webview_guard_settings)
            minHeight = dp(48)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
            setOnClickListener { openProviderSettings() }
        }, row(dp(24)))
        content.addView(Button(context).apply {
            setText(R.string.agentero_webview_guard_close)
            minHeight = dp(48)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
            setOnClickListener { activity.finish() }
        }, row(dp(8)))
        return ScrollView(context).apply {
            isFillViewport = true
            val background = TypedValue()
            if (context.theme.resolveAttribute(android.R.attr.colorBackground, background, true)) {
                setBackgroundColor(background.data)
            }
            addView(content, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
        }
    }

    private fun openProviderSettings() {
        // Query only on the explicit button press, without enumerating packages.
        val provider = try { WebView.getCurrentWebViewPackage()?.packageName } catch (_: Exception) { null }
        if (!provider.isNullOrBlank() && launchSettings(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", provider, null)),
            )) return
        if (!launchSettings(Intent(Settings.ACTION_SETTINGS))) {
            Toast.makeText(activity, R.string.agentero_webview_guard_settings_unavailable, Toast.LENGTH_LONG).show()
        }
    }

    private fun launchSettings(intent: Intent): Boolean = try {
        activity.startActivity(intent)
        true
    } catch (_: ActivityNotFoundException) {
        false
    } catch (_: SecurityException) {
        false
    }

    private fun row(topMargin: Int = 0) = LinearLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT,
    ).apply { this.topMargin = topMargin }

    private fun dp(value: Int): Int = (value * activity.resources.displayMetrics.density + 0.5f).toInt()
}
