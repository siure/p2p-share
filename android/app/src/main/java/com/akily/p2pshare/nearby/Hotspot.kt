package com.akily.p2pshare.nearby

import android.annotation.SuppressLint
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiManager
import android.net.wifi.WifiNetworkSpecifier
import android.os.Build
import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.coroutines.resume

/** A local-only hotspot (no internet sharing) that lives until [stop]. */
@SuppressLint("MissingPermission")
class HotspotHost(private val context: Context) {
    private var reservation: WifiManager.LocalOnlyHotspotReservation? = null

    suspend fun start(): HotspotCredentials? {
        stop()
        val wifi = context.applicationContext.getSystemService(WifiManager::class.java) ?: return null
        return withTimeoutOrNull(20_000) {
            suspendCancellableCoroutine { continuation ->
                val callback = object : WifiManager.LocalOnlyHotspotCallback() {
                    override fun onStarted(started: WifiManager.LocalOnlyHotspotReservation) {
                        // Started after a cancel or timeout: nobody will stop it later.
                        if (!continuation.isActive) {
                            runCatching { started.close() }
                            return
                        }
                        reservation = started
                        continuation.resume(credentials(started))
                    }

                    override fun onFailed(reason: Int) {
                        if (continuation.isActive) continuation.resume(null)
                    }
                }
                runCatching { wifi.startLocalOnlyHotspot(callback, Handler(Looper.getMainLooper())) }
                    .onFailure { if (continuation.isActive) continuation.resume(null) }
            }
        }
    }

    fun stop() {
        runCatching { reservation?.close() }
        reservation = null
    }

    private fun credentials(started: WifiManager.LocalOnlyHotspotReservation): HotspotCredentials? {
        val (ssid, passphrase) = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val config = started.softApConfiguration
            @Suppress("DEPRECATION")
            config.ssid to config.passphrase
        } else {
            @Suppress("DEPRECATION")
            val config = started.wifiConfiguration
            @Suppress("DEPRECATION")
            config?.SSID to config?.preSharedKey
        }
        return HotspotCredentials(ssid?.trim('"') ?: return null, passphrase?.trim('"') ?: return null)
    }
}

/**
 * Joins another device's hotspot and routes this process through it. Sockets
 * opened before joining keep their old network, so the engine must be rebound.
 */
class HotspotClient(context: Context) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private var callback: ConnectivityManager.NetworkCallback? = null

    /** Android asks the person to confirm the connection. */
    suspend fun join(credentials: HotspotCredentials): Boolean {
        leave()
        val specifier = WifiNetworkSpecifier.Builder()
            .setSsid(credentials.ssid)
            .setWpa2Passphrase(credentials.passphrase)
            .build()
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .setNetworkSpecifier(specifier)
            .build()
        return withTimeoutOrNull(60_000) {
            suspendCancellableCoroutine { continuation ->
                val joined = object : ConnectivityManager.NetworkCallback() {
                    override fun onAvailable(network: Network) {
                        connectivity.bindProcessToNetwork(network)
                        if (continuation.isActive) continuation.resume(true)
                    }

                    override fun onUnavailable() {
                        if (continuation.isActive) continuation.resume(false)
                    }
                }
                callback = joined
                connectivity.requestNetwork(request, joined)
            }
        } ?: false
    }

    /** Returns true if a network was left (the engine then needs a rebind). */
    fun leave(): Boolean {
        val joined = callback ?: return false
        callback = null
        connectivity.bindProcessToNetwork(null)
        runCatching { connectivity.unregisterNetworkCallback(joined) }
        return true
    }
}
