package app.agentero.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class AndroidBackgroundPolicyTest {
    private val visible = BackgroundGate(enabled = true, foreground = true, notificationsGranted = true, channelsEnabled = true)
    private val stopped = BackgroundObserved(active = false, pending = false)
    private val running = BackgroundObserved(active = true, pending = false)

    @Test fun splitScreenRemainsForegroundUntilEveryStartedActivityStops() {
        val visibility = StartedActivityVisibility()
        val first = Any()
        val second = Any()
        assertFalse(visibility.foreground)
        assertTrue(visibility.update(first, true))
        assertFalse(visibility.update(first, true))
        assertFalse(visibility.update(second, true))
        assertFalse(visibility.update(first, false))
        assertTrue(visibility.foreground)
        assertFalse(visibility.update(first, false))
        assertTrue(visibility.update(second, false))
        assertFalse(visibility.foreground)
    }

    @Test fun defaultOffAndExplicitGrantsGateLaunch() {
        assertEquals(BackgroundAction.IDLE, AndroidBackgroundPolicy.action(true, visible.copy(enabled = false), stopped))
        assertTrue(AndroidBackgroundPolicy.canEnable(visible))
        assertTrue(AndroidBackgroundPolicy.canEnable(visible.copy(enabled = false)))
        assertFalse(AndroidBackgroundPolicy.canEnable(visible.copy(foreground = false)))
        assertFalse(AndroidBackgroundPolicy.canEnable(visible.copy(notificationsGranted = false)))
        assertFalse(AndroidBackgroundPolicy.canEnable(visible.copy(channelsEnabled = false)))
    }

    @Test fun backgroundMayKeepAnObservedServiceButCannotStartOne() {
        val background = visible.copy(foreground = false)
        assertEquals(BackgroundAction.START, AndroidBackgroundPolicy.action(true, visible, stopped))
        assertEquals(BackgroundAction.IDLE, AndroidBackgroundPolicy.action(true, background, stopped))
        assertEquals(BackgroundAction.KEEP, AndroidBackgroundPolicy.action(true, background, running))
    }

    @Test fun pendingLaunchWaitsWithoutLaunchingTwiceAndStopsOnRelease() {
        val pending = BackgroundObserved(active = false, pending = true)
        assertEquals(BackgroundAction.KEEP, AndroidBackgroundPolicy.action(true, visible, pending))
        assertEquals(BackgroundAction.STOP, AndroidBackgroundPolicy.action(false, visible, pending))
    }

    @Test fun aggregateLeaseReleaseStopsOnlyAtZero() {
        val multiple = BackgroundCounts(agent = 2, sync = 1, transfer = 1)
        val oneAgent = BackgroundCounts(agent = 1)
        assertTrue(multiple.desired(BackgroundLane.SYNC))
        assertEquals(BackgroundAction.KEEP, AndroidBackgroundPolicy.action(oneAgent.desired(BackgroundLane.AGENT), visible, running))
        assertEquals(BackgroundAction.STOP, AndroidBackgroundPolicy.action(oneAgent.desired(BackgroundLane.SYNC), visible, running))
        assertEquals(BackgroundAction.STOP, AndroidBackgroundPolicy.action(BackgroundCounts().desired(BackgroundLane.AGENT), visible, running))
    }

    @Test fun syncAndTransferShareCarrierWithoutOverflowOrAffectingAgent() {
        val maximum = BackgroundCounts(agent = BackgroundCounts.MAX_U32, sync = BackgroundCounts.MAX_U32, transfer = BackgroundCounts.MAX_U32)
        assertTrue(maximum.desired(BackgroundLane.AGENT))
        assertTrue(maximum.desired(BackgroundLane.SYNC))
        assertTrue(BackgroundCounts(transfer = 1).desired(BackgroundLane.SYNC))
        assertFalse(BackgroundCounts(transfer = 1).desired(BackgroundLane.AGENT))
        assertThrows(IllegalArgumentException::class.java) { BackgroundCounts(agent = -1) }
        assertThrows(IllegalArgumentException::class.java) { BackgroundCounts(sync = BackgroundCounts.MAX_U32 + 1) }
    }

    @Test fun optOutOrRevokedNotificationPrerequisitesStopObservedWork() {
        listOf(visible.copy(enabled = false), visible.copy(notificationsGranted = false), visible.copy(channelsEnabled = false)).forEach { gate ->
            assertEquals(BackgroundAction.STOP, AndroidBackgroundPolicy.action(true, gate, running))
        }
        // A disabled data-sync channel need not stop an independently enabled Agent channel.
        assertEquals(BackgroundAction.KEEP, AndroidBackgroundPolicy.action(true, visible, running))
    }

    @Test fun dataSyncTimeoutStopsAndCannotRestartWhileAgentRemainsIndependent() {
        assertEquals(BackgroundAction.STOP, AndroidBackgroundPolicy.action(true, visible, running, expired = true))
        assertEquals(BackgroundAction.IDLE, AndroidBackgroundPolicy.action(true, visible, stopped, expired = true))
        assertEquals(BackgroundAction.KEEP, AndroidBackgroundPolicy.action(true, visible, running, expired = false))
    }

    @Test fun lateApplyCannotOverwriteNewerDesiredWork() {
        val fence = BackgroundGenerationFence()
        assertTrue(fence.accept("2"))
        assertFalse(fence.accept("1"))
        assertFalse(fence.accept("2"))
        assertFalse(fence.accept("0"))
        assertEquals(2uL, fence.lastAccepted)
        assertTrue(fence.accept("18446744073709551615"))
        assertFalse(fence.accept("18446744073709551614"))
        assertEquals(ULong.MAX_VALUE, fence.lastAccepted)
    }

    @Test fun malformedGenerationIsRejectedWithoutAdvancingFence() {
        val fence = BackgroundGenerationFence()
        listOf("", "01", "+1", "-1", " 1", "1 ", "1.0", "١", "18446744073709551616").forEach { raw ->
            assertThrows(IllegalArgumentException::class.java) { fence.accept(raw) }
        }
        assertEquals(0uL, fence.lastAccepted)
        assertTrue(fence.accept("1"))
    }
}
