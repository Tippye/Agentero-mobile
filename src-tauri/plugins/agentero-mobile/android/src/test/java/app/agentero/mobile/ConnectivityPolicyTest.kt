package app.agentero.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class ConnectivityPolicyTest {
    private data class Links(val address: String, val dns: String)

    @Test fun initialSnapshotIsZeroAndRepeatedReadsCannotStartMoreCallbacks() {
        val policy = ConnectivityPolicy<String, Links>()
        assertEquals("0", policy.generation.toString())
        val epoch = requireNotNull(policy.begin())
        assertNull(policy.begin())
        assertTrue(policy.accepts(epoch))
        assertEquals(0L, policy.generation)
    }

    @Test fun handoverCannotBeOverwrittenByLostOfThePreviousDefault() {
        val policy = ConnectivityPolicy<String, Links>()
        val epoch = requireNotNull(policy.begin())
        assertTrue(policy.defaultAvailable(epoch, "wifi-a"))
        assertFalse(policy.defaultAvailable(epoch, "wifi-a"))
        assertTrue(policy.defaultAvailable(epoch, "cell-b"))
        val handedOver = policy.generation
        assertFalse(policy.defaultLost(epoch, "wifi-a"))
        assertEquals(handedOver, policy.generation)
        assertTrue(policy.defaultLost(epoch, "cell-b"))
        assertFalse(policy.defaultLost(epoch, "cell-b"))
    }

    @Test fun nonDefaultLanChangesProduceHintsWithoutInternetOrValidationCapabilities() {
        val policy = ConnectivityPolicy<String, Links>()
        val epoch = requireNotNull(policy.begin())
        policy.defaultAvailable(epoch, "cellular")
        assertTrue(policy.available(epoch, "private-lan"))
        assertTrue(policy.transportsChanged(epoch, "private-lan", setOf(1)))
        assertTrue(policy.linksChanged(epoch, "private-lan", Links("192.168.50.2", "192.168.50.1")))
        assertTrue(policy.lost(epoch, "private-lan"))
        // Losing the LAN never erases the independently tracked cellular default.
        assertFalse(policy.defaultAvailable(epoch, "cellular"))
        assertTrue(policy.defaultLost(epoch, "cellular"))
    }

    @Test fun duplicateNetworkAndLinkDeliveriesFromBothObserversAreCoalesced() {
        val policy = ConnectivityPolicy<String, Links>()
        val epoch = requireNotNull(policy.begin())
        assertTrue(policy.defaultAvailable(epoch, "wifi"))
        assertTrue(policy.available(epoch, "wifi"))
        assertFalse(policy.available(epoch, "wifi"))
        assertTrue(policy.transportsChanged(epoch, "wifi", setOf(1, 4)))
        assertFalse(policy.transportsChanged(epoch, "wifi", setOf(4, 1)))
        val links = Links("10.0.0.2", "10.0.0.1")
        assertTrue(policy.linksChanged(epoch, "wifi", links))
        val snapshot = policy.generation
        assertFalse(policy.linksChanged(epoch, "wifi", links.copy()))
        assertEquals(snapshot, policy.generation)
    }

    @Test fun addressDnsAndTransportChangesWakeEvenWhenTheNetworkIdentityIsStable() {
        val policy = ConnectivityPolicy<String, Links>()
        val epoch = requireNotNull(policy.begin())
        policy.available(epoch, "wifi")
        policy.linksChanged(epoch, "wifi", Links("10.0.0.2", "10.0.0.1"))
        assertTrue(policy.linksChanged(epoch, "wifi", Links("10.0.0.3", "10.0.0.1")))
        assertTrue(policy.linksChanged(epoch, "wifi", Links("10.0.0.3", "10.0.0.4")))
        assertTrue(policy.transportsChanged(epoch, "wifi", setOf(1)))
        assertTrue(policy.transportsChanged(epoch, "wifi", setOf(4)))
    }

    @Test fun oneNetworksLossDoesNotForgetAnotherOrAcceptLatePropertiesForTheLostOne() {
        val policy = ConnectivityPolicy<String, Links>()
        val epoch = requireNotNull(policy.begin())
        policy.available(epoch, "a")
        policy.available(epoch, "b")
        assertTrue(policy.lost(epoch, "a"))
        assertFalse(policy.lost(epoch, "a"))
        assertFalse(policy.linksChanged(epoch, "a", Links("old", "old")))
        assertFalse(policy.transportsChanged(epoch, "a", setOf(1)))
        assertFalse(policy.available(epoch, "b"))
        assertTrue(policy.linksChanged(epoch, "b", Links("current", "current")))
    }

    @Test fun defaultAndAllNetworkMembershipRetainSharedLinkBaselineUntilBothAreGone() {
        val policy = ConnectivityPolicy<String, Links>()
        val epoch = requireNotNull(policy.begin())
        val links = Links("address", "dns")
        policy.defaultAvailable(epoch, "wifi")
        policy.available(epoch, "wifi")
        policy.linksChanged(epoch, "wifi", links)
        policy.lost(epoch, "wifi")
        assertFalse(policy.linksChanged(epoch, "wifi", links))
        policy.defaultLost(epoch, "wifi")
        assertFalse(policy.linksChanged(epoch, "wifi", links))
        policy.available(epoch, "wifi")
        assertTrue(policy.linksChanged(epoch, "wifi", links))
    }

    @Test fun replacingADefaultThatIsNotOtherwiseTrackedReleasesItsOldLinkBaseline() {
        val policy = ConnectivityPolicy<String, Links>()
        val epoch = requireNotNull(policy.begin())
        val links = Links("address", "dns")
        policy.defaultAvailable(epoch, "a")
        policy.linksChanged(epoch, "a", links)
        policy.defaultAvailable(epoch, "b")
        policy.defaultAvailable(epoch, "a")
        assertTrue(policy.linksChanged(epoch, "a", links))
    }

    @Test fun stopFencesQueuedCallbacksAndRestartPreservesProcessGeneration() {
        val policy = ConnectivityPolicy<String, Links>()
        val oldEpoch = requireNotNull(policy.begin())
        policy.available(oldEpoch, "old")
        val snapshot = policy.generation
        policy.stop()
        assertFalse(policy.available(oldEpoch, "late"))
        assertFalse(policy.defaultAvailable(oldEpoch, "late"))
        val currentEpoch = requireNotNull(policy.begin())
        assertTrue(currentEpoch > oldEpoch)
        assertFalse(policy.lost(oldEpoch, "old"))
        assertFalse(policy.linksChanged(oldEpoch, "old", Links("stale", "stale")))
        assertEquals(snapshot, policy.generation)
        assertTrue(policy.available(currentEpoch, "new"))
        assertEquals(snapshot + 1, policy.generation)
    }

    @Test fun generationExhaustionRejectsChangeWithoutWrappingOrReusingTheCounter() {
        val policy = ConnectivityPolicy<String, Links>(Long.MAX_VALUE - 1)
        val epoch = requireNotNull(policy.begin())
        assertTrue(policy.available(epoch, "last"))
        assertEquals("9223372036854775807", policy.generation.toString())
        assertFalse(policy.available(epoch, "last"))
        assertThrows(IllegalStateException::class.java) { policy.available(epoch, "overflow") }
        assertEquals(Long.MAX_VALUE, policy.generation)
        policy.stop()
        assertFalse(policy.available(epoch, "late"))
        assertThrows(IllegalArgumentException::class.java) { ConnectivityPolicy<String, Links>(-1) }
    }
}
