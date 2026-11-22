package app.agentero.mobile

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Handler
import android.os.Looper
import android.util.Log
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.lang.ref.WeakReference
import java.util.Collections
import java.util.WeakHashMap

/** Strongly owned by the plugin, weakly owned by the process monitor. */
internal class ConnectivityEventRelay(plugin: Plugin) {
    private val target = WeakReference(plugin)

    fun changed(generation: String) {
        target.get()?.trigger("connectivity-changed", JSObject().apply {
            put("generation", generation)
        })
    }
}

/** Passive process observer; it neither owns sockets nor retains an Activity. */
internal class ConnectivityMonitor private constructor(context: Context) {
    private val connectivity = requireNotNull(
        context.applicationContext.getSystemService(ConnectivityManager::class.java),
    ) { "Connectivity monitoring unavailable" }
    private val handler = Handler(Looper.getMainLooper())
    private val policy = ConnectivityPolicy<Network, LinkProperties>()
    private val relays = Collections.newSetFromMap(WeakHashMap<ConnectivityEventRelay, Boolean>())
    private var registrations: Registrations? = null
    private var failure: Exception? = null

    private class Registrations(
        val default: ConnectivityManager.NetworkCallback,
        val all: ConnectivityManager.NetworkCallback,
        var defaultRegistered: Boolean = false,
        var allRegistered: Boolean = false,
    )

    /** Rust registers its channel before calling this method, then fences the returned generation. */
    fun state(relay: ConnectivityEventRelay): JSObject {
        checkMainThread()
        failure?.let { throw IllegalStateException("Connectivity monitoring unavailable", it) }
        relays.add(relay)
        start()
        return JSObject().apply { put("generation", policy.generation.toString()) }
    }

    private fun start() {
        val epoch = policy.begin() ?: return
        val pair = Registrations(callback(epoch, true), callback(epoch, false))
        registrations = pair
        try {
            connectivity.registerDefaultNetworkCallback(pair.default, handler)
            pair.defaultRegistered = true
            // A LAN without WAN may not be the default network. Observe it without an
            // INTERNET/VALIDATED requirement, and include application-accessible VPNs.
            val request = NetworkRequest.Builder()
                .removeCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
                .build()
            connectivity.registerNetworkCallback(request, pair.all, handler)
            pair.allRegistered = true
        } catch (error: Exception) {
            try {
                stop()
            } catch (cleanup: Exception) {
                error.addSuppressed(cleanup)
            }
            throw error
        }
    }

    private fun callback(epoch: Long, default: Boolean) = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = update(epoch) {
            if (default) policy.defaultAvailable(epoch, network) else policy.available(epoch, network)
        }

        override fun onLost(network: Network) = update(epoch) {
            if (default) policy.defaultLost(epoch, network) else policy.lost(epoch, network)
        }

        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) = update(epoch) {
            policy.transportsChanged(epoch, network, OBSERVED_TRANSPORTS.filter { capabilities.hasTransport(it) }.toSet())
        }

        override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) = update(epoch) {
            // ConnectivityManager delivers a new snapshot; retain it without mutating it.
            policy.linksChanged(epoch, network, properties)
        }
    }

    private fun update(epoch: Long, change: () -> Boolean) {
        checkMainThread()
        if (!policy.accepts(epoch)) return
        val changed = try {
            change()
        } catch (error: Exception) {
            // Generation exhaustion must stop observation, never wrap or reuse a hint.
            failure = error
            try { stop() } catch (cleanup: Exception) { error.addSuppressed(cleanup) }
            Log.e("AgenteroConnectivity", "Connectivity monitoring stopped", error)
            return
        }
        if (changed) {
            val generation = policy.generation.toString()
            relays.toList().forEach { it.changed(generation) }
        }
    }

    /** Explicit process-owner teardown only; never called on Activity destruction or FGS opt-out. */
    fun stop() {
        checkMainThread()
        policy.stop() // Fence already queued callbacks before unregistering either observer.
        val pair = registrations
        registrations = null
        var cleanupFailure: Exception? = null
        if (pair != null) {
            for ((callback, registered) in listOf(
                pair.default to pair.defaultRegistered,
                pair.all to pair.allRegistered,
            )) {
                if (!registered) continue
                try {
                    connectivity.unregisterNetworkCallback(callback)
                } catch (error: Exception) {
                    if (cleanupFailure == null) cleanupFailure = error else cleanupFailure.addSuppressed(error)
                }
            }
        }
        cleanupFailure?.let {
            failure = it // Unknown OS ownership must not allocate replacement callbacks.
            throw it
        }
    }

    private fun checkMainThread() {
        check(Looper.myLooper() == Looper.getMainLooper()) { "Connectivity monitoring requires the main thread" }
    }

    companion object {
        private val OBSERVED_TRANSPORTS = intArrayOf(
            NetworkCapabilities.TRANSPORT_CELLULAR,
            NetworkCapabilities.TRANSPORT_WIFI,
            NetworkCapabilities.TRANSPORT_BLUETOOTH,
            NetworkCapabilities.TRANSPORT_ETHERNET,
            NetworkCapabilities.TRANSPORT_VPN,
            NetworkCapabilities.TRANSPORT_WIFI_AWARE,
            NetworkCapabilities.TRANSPORT_LOWPAN,
            NetworkCapabilities.TRANSPORT_USB,
        )
        @Volatile private var instance: ConnectivityMonitor? = null

        fun get(context: Context): ConnectivityMonitor = instance ?: synchronized(this) {
            instance ?: ConnectivityMonitor(context.applicationContext).also { instance = it }
        }
    }
}
