package app.agentero.mobile

/** Engine information comes only from the native provider's default user agent. */
internal object WebViewCompatibilityPolicy {
    const val MINIMUM_CHROME_MAJOR = 111
    private val chromeVersion = Regex("Chrome/([0-9]+)\\.([0-9]+)\\.([0-9]+)\\.([0-9]+)")
    private val whitespace = Regex("\\s+")

    enum class Status { SUPPORTED, UNSUPPORTED, UNKNOWN }

    data class Decision(val status: Status, val major: Int? = null) {
        val blocksContent: Boolean get() = status == Status.UNSUPPORTED
    }

    fun evaluate(readDefaultUserAgent: () -> String?): Decision {
        val ua = try { readDefaultUserAgent() } catch (_: Exception) { null }
        if (ua.isNullOrBlank() || ua.length > 4096 || ua.any { it.isISOControl() }) return Decision(Status.UNKNOWN)
        val tokens = ua.trim().split(whitespace).filter { it.startsWith("Chrome/") }
        if (tokens.size != 1) return Decision(Status.UNKNOWN)
        val match = chromeVersion.matchEntire(tokens.single()) ?: return Decision(Status.UNKNOWN)
        val parts = match.groupValues.drop(1).map { it.toIntOrNull() }
        if (parts.any { it == null } || parts.first() == 0) return Decision(Status.UNKNOWN)
        val major = requireNotNull(parts.first())
        return Decision(if (major < MINIMUM_CHROME_MAJOR) Status.UNSUPPORTED else Status.SUPPORTED, major)
    }
}
