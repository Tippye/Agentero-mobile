package app.agentero.mobile

import java.util.Collections
import java.util.WeakHashMap

/** Identity lifetimes can overlap in split screen; stopping one window is not background. */
internal class StartedActivityVisibility {
    private val started = Collections.newSetFromMap(WeakHashMap<Any, Boolean>())
    val foreground: Boolean get() = started.isNotEmpty()

    fun update(owner: Any, visible: Boolean): Boolean {
        val before = foreground
        if (visible) started.add(owner) else started.remove(owner)
        return before != foreground
    }
}

/** Native counts are desired aggregate leases from Rust, never persisted task data. */
internal data class BackgroundCounts(
    val agent: Long = 0,
    val sync: Long = 0,
    val transfer: Long = 0,
) {
    init {
        require(listOf(agent, sync, transfer).all { it in 0..MAX_U32 }) {
            "Background counts must be unsigned 32-bit values"
        }
    }

    fun desired(lane: BackgroundLane): Boolean = when (lane) {
        BackgroundLane.AGENT -> agent > 0
        BackgroundLane.SYNC -> sync > 0 || transfer > 0
    }

    companion object {
        const val MAX_U32 = 4_294_967_295L
    }
}

/** JS cannot represent u64 safely; reject non-canonical strings and stale deliveries. */
internal class BackgroundGenerationFence {
    var lastAccepted: ULong = 0uL
        private set

    fun accept(raw: String): Boolean {
        require(raw == "0" || (raw.isNotEmpty() && raw[0] in '1'..'9' && raw.all { it in '0'..'9' })) {
            "Background generation must be a canonical unsigned decimal string"
        }
        val next = requireNotNull(raw.toULongOrNull()) { "Background generation exceeds u64" }
        if (next <= lastAccepted) return false
        lastAccepted = next
        return true
    }
}

internal enum class BackgroundLane { AGENT, SYNC }
internal enum class BackgroundAction { START, KEEP, STOP, IDLE }

internal data class BackgroundGate(
    val enabled: Boolean,
    val foreground: Boolean,
    val notificationsGranted: Boolean,
    val channelsEnabled: Boolean,
)

/** Pending launch is distinct from observed startForeground success. */
internal data class BackgroundObserved(val active: Boolean, val pending: Boolean)

internal object AndroidBackgroundPolicy {
    fun canEnable(gate: BackgroundGate): Boolean =
        gate.foreground && gate.notificationsGranted && gate.channelsEnabled

    fun action(
        desired: Boolean,
        gate: BackgroundGate,
        observed: BackgroundObserved,
        expired: Boolean = false,
    ): BackgroundAction {
        val permitted = gate.enabled && gate.notificationsGranted && gate.channelsEnabled && !expired
        if (!desired || !permitted) {
            return if (observed.active || observed.pending) BackgroundAction.STOP else BackgroundAction.IDLE
        }
        if (observed.active || observed.pending) return BackgroundAction.KEEP
        return if (gate.foreground) BackgroundAction.START else BackgroundAction.IDLE
    }
}
