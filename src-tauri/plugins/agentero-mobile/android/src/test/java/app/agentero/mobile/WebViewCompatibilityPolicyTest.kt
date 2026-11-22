package app.agentero.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WebViewCompatibilityPolicyTest {
    private fun ua(version: String) = "Mozilla/5.0 (Linux; Android 13; device; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/$version Mobile Safari/537.36"
    private fun evaluate(value: String?) = WebViewCompatibilityPolicy.evaluate { value }

    @Test fun knownOldEngineIsBlockedBeforeItsApplicationCanRun() {
        for (version in listOf("37.0.2062.117", "110.0.5481.154")) {
            val result = evaluate(ua(version))
            assertEquals(WebViewCompatibilityPolicy.Status.UNSUPPORTED, result.status)
            assertTrue(result.blocksContent)
            assertEquals(version.substringBefore('.').toInt(), result.major)
        }
    }

    @Test fun chromium111BoundaryIsSupportedWithoutRequestingContentReplacement() {
        val result = evaluate(ua("111.0.5563.0"))
        assertEquals(WebViewCompatibilityPolicy.Status.SUPPORTED, result.status)
        assertEquals(111, result.major)
        assertFalse(result.blocksContent)
    }

    @Test fun newStableAndBetaEngineUasRetainNormalSdkContent() {
        for (version in listOf("112.0.5615.28", "120.0.0.0", "150.0.7777.1")) {
            val result = evaluate(ua(version))
            assertEquals(WebViewCompatibilityPolicy.Status.SUPPORTED, result.status)
            assertFalse(result.blocksContent)
        }
    }

    @Test fun oemMarketingVersionDoesNotOverrideTheActualEngineToken() {
        val result = evaluate("HuaweiBrowser/16.0.0.300 " + ua("120.0.6099.43"))
        assertEquals(WebViewCompatibilityPolicy.Status.SUPPORTED, result.status)
        assertEquals(120, result.major)
    }

    @Test fun missingAndMalformedEvidenceIsUnknownRatherThanKnownCompatibleOrBlocked() {
        for (value in listOf(null, "", " ", "Mozilla/5.0 Version/16.0 Safari/537.36",
            ua("0.0.0.0"), ua("-110.0.0.0"), ua("110"), ua("110.0.1"),
            ua("110.0.1.2.3"), ua("110.0.x.2"), ua("110.0.1.2-beta"),
            "NotChrome/110.0.1.2", "Chrome/110.0.1.2;", "Chrome/110.0.1.2\n")) {
            val result = evaluate(value)
            assertEquals(value, WebViewCompatibilityPolicy.Status.UNKNOWN, result.status)
            assertNull(result.major)
            assertFalse(result.blocksContent)
        }
    }

    @Test fun overflowInAnyVersionPartCannotBeMistakenForAnOldEngine() {
        for (version in listOf("999999999999999999.0.0.0", "110.999999999999999999.0.0",
            "110.0.999999999999999999.0", "110.0.0.999999999999999999")) {
            val result = evaluate(ua(version))
            assertEquals(WebViewCompatibilityPolicy.Status.UNKNOWN, result.status)
            assertFalse(result.blocksContent)
        }
    }

    @Test fun duplicateEvenConflictingOrMalformedChromeTokensAreUnknown() {
        for (extra in listOf("Chrome/110.0.1.2", "Chrome/120.0.1.2", "Chrome/invalid")) {
            val result = evaluate(ua("110.0.1.2") + " " + extra)
            assertEquals(WebViewCompatibilityPolicy.Status.UNKNOWN, result.status)
            assertFalse(result.blocksContent)
        }
    }

    @Test fun evidenceLengthIsBoundedAt4096WithoutTruncatingIntoAValidToken() {
        val valid = "Chrome/110.0.1.2"
        val boundary = "x".repeat(4096 - valid.length - 1) + " " + valid
        assertTrue(evaluate(boundary).blocksContent)
        assertEquals(WebViewCompatibilityPolicy.Status.UNKNOWN, evaluate("x" + boundary).status)
        assertEquals(WebViewCompatibilityPolicy.Status.UNKNOWN, evaluate("Chrome/" + "1".repeat(4096)).status)
    }

    @Test fun nativeReaderFailureIsUnknownAndDoesNotBlockNormalStartup() {
        val result = WebViewCompatibilityPolicy.evaluate { throw IllegalStateException("provider unavailable") }
        assertEquals(WebViewCompatibilityPolicy.Status.UNKNOWN, result.status)
        assertFalse(result.blocksContent)
    }
}
