package app.agentero.mobile

import android.content.Context
import android.util.Log
import java.io.File
import java.io.IOException

/**
 * Materializes the bundled reflow fallback font into the cache directory the
 * webview already reads.
 *
 * The reflow engine typesets translated PDFs with embedded fonts. It normally
 * fetches them from a pinned public host and caches the result, but that host is
 * routinely unreachable from a phone, and typesetting then fails outright. One
 * CJK serif ships in the APK instead: it also covers the Latin glyphs, so it can
 * stand in for every family. Copied here rather than resolved by the webview
 * because APK assets are owned by AssetManager and are not readable as files.
 *
 * The copy is versioned by `fonts/version.txt` and replaced atomically, so an
 * upgraded APK picks up new font bytes without leaving a half-written file.
 */
internal object ReflowFontAssets {
    private const val TAG = "AgenteroReflowFonts"
    private const val ASSET = "fonts/SourceHanSerifCN-Regular.ttf"
    private const val VERSION_ASSET = "fonts/version.txt"
    private const val DIR = "pdf-reflow-fallback"
    private const val FILE_NAME = "fallback.ttf"
    private val lock = Any()

    /**
     * Best-effort: a missing font only degrades typesetting, so failures are
     * logged and swallowed instead of surfacing a startup error.
     */
    fun warm(context: Context) {
        Thread({
            try {
                synchronized(lock) { materialize(context) }
            } catch (error: Exception) {
                Log.w(TAG, "fallback font unavailable: ${error.message}")
            }
        }, "agentero-reflow-fonts").start()
    }

    /** Absolute path the webview reads, whether or not the copy succeeded yet. */
    fun path(context: Context): String =
        File(File(context.cacheDir, DIR), FILE_NAME).absolutePath

    private fun materialize(context: Context): File {
        val dir = File(context.cacheDir, DIR).apply {
            if (!isDirectory && !mkdirs()) throw IOException("cannot create font directory")
        }
        val target = File(dir, FILE_NAME)
        val stamp = File(dir, "version.txt")
        val version = assetText(context, VERSION_ASSET)
        if (version != null && target.isFile && target.length() > 0 &&
            stamp.isFile && stamp.readText().trim() == version
        ) {
            return target
        }
        if (version == null) throw IOException("bundled font version is missing")
        val temp = File(dir, "fallback.ttf.tmp")
        context.assets.open(ASSET).use { input ->
            temp.outputStream().use { output -> input.copyTo(output) }
        }
        if (temp.length() <= 0) {
            temp.delete()
            throw IOException("bundled fallback font is empty")
        }
        if (target.exists() && !target.delete()) {
            temp.delete()
            throw IOException("cannot replace stale fallback font")
        }
        if (!temp.renameTo(target)) {
            temp.delete()
            throw IOException("cannot commit fallback font")
        }
        stamp.writeText(version)
        Log.i(TAG, "fallback font materialized (${target.length()} bytes)")
        return target
    }

    private fun assetText(context: Context, name: String): String? = try {
        context.assets.open(name).bufferedReader().use { it.readText().trim() }
    } catch (_: IOException) {
        null
    }
}