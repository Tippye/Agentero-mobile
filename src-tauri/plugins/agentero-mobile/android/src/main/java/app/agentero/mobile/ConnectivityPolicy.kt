package app.agentero.mobile

/** Process-local topology reducer. Keys and link details never cross the native bridge. */
internal class ConnectivityPolicy<Key : Any, Links : Any>(initialGeneration: Long = 0) {
    init { require(initialGeneration >= 0) }

    var generation: Long = initialGeneration
        private set
    private var epoch = 0L
    private var running = false
    private var defaultNetwork: Key? = null
    private val networks = mutableSetOf<Key>()
    private val transports = mutableMapOf<Key, Set<Int>>()
    private val links = mutableMapOf<Key, Links>()

    /** A repeated attach/read does not allocate another pair of OS callbacks. */
    fun begin(): Long? {
        if (running) return null
        check(epoch < Long.MAX_VALUE) { "Connectivity monitor epoch exhausted" }
        epoch += 1
        running = true
        return epoch
    }

    fun accepts(callbackEpoch: Long): Boolean = running && callbackEpoch == epoch

    fun stop() {
        running = false
        defaultNetwork = null
        networks.clear()
        transports.clear()
        links.clear()
    }

    fun defaultAvailable(callbackEpoch: Long, network: Key): Boolean {
        if (!accepts(callbackEpoch) || defaultNetwork == network) return false
        advance()
        val previous = defaultNetwork
        defaultNetwork = network
        if (previous != null) forgetIfUnused(previous)
        return true
    }

    fun defaultLost(callbackEpoch: Long, network: Key): Boolean {
        if (!accepts(callbackEpoch) || defaultNetwork != network) return false
        advance()
        defaultNetwork = null
        forgetIfUnused(network)
        return true
    }

    fun available(callbackEpoch: Long, network: Key): Boolean {
        if (!accepts(callbackEpoch) || network in networks) return false
        advance()
        networks.add(network)
        return true
    }

    fun lost(callbackEpoch: Long, network: Key): Boolean {
        if (!accepts(callbackEpoch) || network !in networks) return false
        advance()
        networks.remove(network)
        forgetIfUnused(network)
        return true
    }

    /** Only transport topology matters here, not RSSI, bandwidth or Internet validation. */
    fun transportsChanged(callbackEpoch: Long, network: Key, value: Set<Int>): Boolean {
        if (!tracked(callbackEpoch, network) || transports[network] == value) return false
        advance()
        transports[network] = value.toSet()
        return true
    }

    /** Link equality observes route/DNS/address changes locally without exporting their contents. */
    fun linksChanged(callbackEpoch: Long, network: Key, value: Links): Boolean {
        if (!tracked(callbackEpoch, network) || links[network] == value) return false
        advance()
        links[network] = value
        return true
    }

    private fun tracked(callbackEpoch: Long, network: Key): Boolean =
        accepts(callbackEpoch) && (defaultNetwork == network || network in networks)

    private fun forgetIfUnused(network: Key) {
        if (network == defaultNetwork || network in networks) return
        transports.remove(network)
        links.remove(network)
    }

    private fun advance() {
        check(generation < Long.MAX_VALUE) { "Connectivity generation exhausted" }
        generation += 1
    }
}
