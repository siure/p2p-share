package com.akily.p2pshare.nearby

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.net.wifi.WifiManager
import android.os.Build
import android.provider.Settings
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.akily.p2pshare.bridge.TransferEngine
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

data class NearbyPeer(
    val id: String,
    val name: String,
    val ticket: String,
    val lan: Boolean,
    val bluetoothAddress: String?,
)

/**
 * Android half of nearby sharing. The Rust engine handles mDNS; this class adds
 * Bluetooth LE discovery and, when two devices share no network, a hotspot.
 * Must be used from the main thread.
 */
class NearbyManager(
    private val context: Context,
    private val scope: CoroutineScope,
    private val engine: TransferEngine,
) {
    var peers by mutableStateOf<List<NearbyPeer>>(emptyList())
        private set

    val deviceName: String =
        Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() } ?: Build.MODEL

    // id -> sightings; LAN data wins because its ticket is not compacted.
    private val lan = mutableMapOf<String, Pair<String, String>>()
    private val ble = mutableMapOf<String, Triple<String, String, String>>()
    private val bleIds = mutableMapOf<String, String>()
    private var scanner: BleScanner? = null
    private var browsePoll: Job? = null
    private var advertiser: BleAdvertiser? = null
    private val host = HotspotHost(context)
    private val client = HotspotClient(context)
    private var multicast: WifiManager.MulticastLock? = null

    init {
        engine.setDeviceName(deviceName)
    }

    fun startBrowsing() {
        if (browsePoll != null) return
        holdMulticast(true)
        engine.startBrowse()
        browsePoll = scope.launch {
            while (isActive) {
                while (true) {
                    val record = engine.pollNearby() ?: break
                    val ticket = record.ticket
                    if (ticket == null) lan.remove(record.peerId)
                    else lan[record.peerId] = (record.name ?: "Unknown device") to ticket
                }
                publish()
                delay(250)
            }
        }
        if (!bluetoothAllowed()) return
        scanner = BleScanner(context, scope, onFound = { address, beacon ->
            val id = Tickets.peerId(beacon.ticket) ?: return@BleScanner
            bleIds[address] = id
            ble[id] = Triple(address, beacon.name, beacon.ticket)
            publish()
        }, onLost = { address ->
            bleIds.remove(address)?.let(ble::remove)
            publish()
        }).takeIf { it.start() }
    }

    fun stopBrowsing() {
        browsePoll?.cancel()
        browsePoll = null
        engine.stopBrowse()
        scanner?.stop()
        scanner = null
        lan.clear()
        ble.clear()
        bleIds.clear()
        publish()
        holdMulticast(false)
    }

    private fun publish() {
        peers = (lan.keys + ble.keys).distinct().mapNotNull { id ->
            val viaLan = lan[id]
            val viaBle = ble[id]
            val (name, ticket) = viaLan ?: viaBle?.let { it.second to it.third } ?: return@mapNotNull null
            NearbyPeer(id, name, ticket, lan = viaLan != null, bluetoothAddress = viaBle?.first)
        }.sortedBy { it.name.lowercase() }
    }

    /** Receiver side, alongside `engine.startReceiveNearby`. False if Bluetooth is unavailable. */
    fun startAdvertising(onStatus: (String) -> Unit): Boolean {
        holdMulticast(true)
        if (!bluetoothAllowed()) return false
        val started = BleAdvertiser(context) {
            scope.launch {
                onStatus("A nearby device has no shared network. Starting a hotspot...")
                advertiser?.setHotspot(host.start())
            }
        }
        advertiser = started.takeIf { it.start() }
        return advertiser != null
    }

    /** Called with each `beacon` event of the nearby receive. */
    fun updateBeacon(json: String) {
        advertiser?.updateBeacon(json)
    }

    fun stopAdvertising() {
        advertiser?.stop()
        advertiser = null
        host.stop()
        holdMulticast(false)
    }

    /**
     * Find a ticket that reaches [peer]. LAN and relay tickets work as they are;
     * otherwise one device hosts a hotspot that the other joins. Throws when the
     * peer cannot be reached.
     */
    suspend fun reach(peer: NearbyPeer, onStatus: (String) -> Unit): String {
        val address = peer.bluetoothAddress
        if (peer.lan || address == null) return peer.ticket
        val beacon = GattClient.readBeacon(context, address)
            ?: error("${peer.name} is no longer reachable over Bluetooth.")
        if (engine.hasRelay() && Tickets.hasRelay(beacon.ticket)) return beacon.ticket

        onStatus("No shared network with ${peer.name}. Asking it to start a hotspot...")
        if (GattClient.write(context, address, NearbyUuids.HOTSPOT, byteArrayOf(1))) {
            val credentials = poll(20_000) { GattClient.readHotspot(context, address) }
                ?: error("${peer.name} did not start its hotspot.")
            onStatus("Joining ${credentials.ssid}...")
            if (!client.join(credentials)) error("Could not join the hotspot of ${peer.name}.")
            engine.rebind()
        } else {
            onStatus("Starting a hotspot for ${peer.name}...")
            val credentials = host.start() ?: error("Could not start a hotspot. Turn on Wi-Fi and try again.")
            val asked = GattClient.write(context, address, NearbyUuids.JOIN, credentials.toJson().toByteArray())
            if (!asked) error("${peer.name} cannot join a hotspot.")
            onStatus("Waiting for ${peer.name} to join...")
        }
        // The receiver's ticket gains the shared network's address once it sees it.
        return poll(30_000) {
            GattClient.readBeacon(context, address)?.ticket?.takeIf { it != beacon.ticket }
        } ?: beacon.ticket
    }

    /** Undo [reach] once the transfer is over. */
    fun releaseNetwork() {
        host.stop()
        if (client.leave()) engine.rebind()
    }

    private suspend fun <T> poll(timeoutMs: Long, attempt: suspend () -> T?): T? {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (System.currentTimeMillis() < deadline) {
            attempt()?.let { return it }
            delay(1_500)
        }
        return null
    }

    /** Bluetooth calls throw without these grants; mDNS needs none. */
    private fun bluetoothAllowed(): Boolean {
        val needed = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            listOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_ADVERTISE,
                Manifest.permission.BLUETOOTH_CONNECT)
        } else {
            listOf(Manifest.permission.ACCESS_FINE_LOCATION)
        }
        return needed.all { context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }
    }

    /** Android drops multicast (mDNS) packets unless an app holds this lock. */
    private fun holdMulticast(hold: Boolean) {
        if (hold && multicast == null) {
            val wifi = context.applicationContext.getSystemService(WifiManager::class.java)
            multicast = wifi?.createMulticastLock("p2p-share-nearby")?.apply {
                setReferenceCounted(false)
                acquire()
            }
        } else if (!hold && advertiser == null && browsePoll == null) {
            multicast?.release()
            multicast = null
        }
    }

    fun close() {
        stopBrowsing()
        stopAdvertising()
        releaseNetwork()
    }
}
