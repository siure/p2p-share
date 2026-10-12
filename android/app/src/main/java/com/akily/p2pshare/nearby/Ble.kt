package com.akily.p2pshare.nearby

import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.os.Build
import android.os.ParcelUuid
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject
import java.util.UUID

/** GATT contract shared with the Rust core (`crates/core/src/nearby/mod.rs`). */
object NearbyUuids {
    val SERVICE: UUID = UUID.fromString("6d1c0001-5a7b-4f6e-9a39-2b8f0c1d7e10")
    val BEACON: UUID = UUID.fromString("6d1c0002-5a7b-4f6e-9a39-2b8f0c1d7e10")
    val HOTSPOT: UUID = UUID.fromString("6d1c0003-5a7b-4f6e-9a39-2b8f0c1d7e10")
    val JOIN: UUID = UUID.fromString("6d1c0004-5a7b-4f6e-9a39-2b8f0c1d7e10")
}

data class Beacon(val name: String, val ticket: String) {
    companion object {
        fun parse(bytes: ByteArray): Beacon? = runCatching {
            val json = JSONObject(String(bytes, Charsets.UTF_8))
            if (json.optInt("v") != 1) return null
            Beacon(json.getString("n"), json.getString("t"))
        }.getOrNull()
    }
}

data class HotspotCredentials(val ssid: String, val passphrase: String) {
    fun toJson(): String = JSONObject().put("s", ssid).put("p", passphrase).toString()

    companion object {
        fun parse(bytes: ByteArray): HotspotCredentials? = runCatching {
            val json = JSONObject(String(bytes, Charsets.UTF_8))
            HotspotCredentials(json.getString("s"), json.getString("p"))
        }.getOrNull()
    }
}

private fun bluetooth(context: Context) =
    context.getSystemService(BluetoothManager::class.java)?.adapter?.takeIf { it.isEnabled }

/**
 * Receiver side: advertises the service and serves the beacon. A write to the
 * hotspot characteristic calls [onHotspotRequest]; its result is served on reads.
 */
@SuppressLint("MissingPermission")
class BleAdvertiser(
    private val context: Context,
    private val onHotspotRequest: () -> Unit,
) {
    @Volatile private var beacon = ByteArray(0)
    @Volatile private var hotspot = ByteArray(0)
    private var server: BluetoothGattServer? = null
    private val advertiseCallback = object : AdvertiseCallback() {}

    /** Returns false when Bluetooth is off or unsupported. */
    fun start(): Boolean {
        val adapter = bluetooth(context) ?: return false
        val advertiser = adapter.bluetoothLeAdvertiser ?: return false
        val manager = context.getSystemService(BluetoothManager::class.java)
        val gatt = manager.openGattServer(context, serverCallback) ?: return false
        val service = BluetoothGattService(NearbyUuids.SERVICE, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        val read = BluetoothGattCharacteristic.PROPERTY_READ
        val write = BluetoothGattCharacteristic.PROPERTY_WRITE
        val readPerm = BluetoothGattCharacteristic.PERMISSION_READ
        val writePerm = BluetoothGattCharacteristic.PERMISSION_WRITE
        service.addCharacteristic(BluetoothGattCharacteristic(NearbyUuids.BEACON, read, readPerm))
        service.addCharacteristic(BluetoothGattCharacteristic(NearbyUuids.HOTSPOT, read or write, readPerm or writePerm))
        service.addCharacteristic(BluetoothGattCharacteristic(NearbyUuids.JOIN, write, writePerm))
        gatt.addService(service)
        server = gatt
        advertiser.startAdvertising(
            AdvertiseSettings.Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                .setConnectable(true)
                .build(),
            AdvertiseData.Builder().addServiceUuid(ParcelUuid(NearbyUuids.SERVICE)).build(),
            advertiseCallback,
        )
        return true
    }

    fun updateBeacon(json: String) {
        beacon = json.toByteArray(Charsets.UTF_8)
    }

    fun setHotspot(credentials: HotspotCredentials?) {
        hotspot = credentials?.toJson()?.toByteArray(Charsets.UTF_8) ?: ByteArray(0)
    }

    fun stop() {
        runCatching { bluetooth(context)?.bluetoothLeAdvertiser?.stopAdvertising(advertiseCallback) }
        runCatching { server?.close() }
        server = null
    }

    private val serverCallback = object : BluetoothGattServerCallback() {
        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic,
        ) {
            val value = when (characteristic.uuid) {
                NearbyUuids.BEACON -> beacon
                NearbyUuids.HOTSPOT -> hotspot
                else -> ByteArray(0)
            }
            // Long values are read in MTU-sized pieces at increasing offsets.
            val piece = value.copyOfRange(offset.coerceAtMost(value.size), value.size)
            server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, piece)
        }

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray?,
        ) {
            // Joining a network would rebind the listening endpoint, so Android hosts instead.
            val status = if (characteristic.uuid == NearbyUuids.HOTSPOT) {
                onHotspotRequest()
                BluetoothGatt.GATT_SUCCESS
            } else {
                BluetoothGatt.GATT_REQUEST_NOT_SUPPORTED
            }
            if (responseNeeded) server?.sendResponse(device, requestId, status, offset, null)
        }
    }
}

/**
 * Sender side: scans for receivers and reads each new one's beacon. A peer is
 * lost once it has not been heard for [LOST_AFTER_MS].
 */
@SuppressLint("MissingPermission")
class BleScanner(
    private val context: Context,
    private val scope: CoroutineScope,
    private val onFound: (address: String, beacon: Beacon) -> Unit,
    private val onLost: (address: String) -> Unit,
) {
    private val lastHeard = mutableMapOf<String, Long>()
    private val known = mutableSetOf<String>()
    private val reading = mutableSetOf<String>()
    private var sweeper: Job? = null

    private val callback = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            val address = result.device.address
            scope.launch { heard(address) }
        }
    }

    fun start(): Boolean {
        val scanner = bluetooth(context)?.bluetoothLeScanner ?: return false
        scanner.startScan(
            listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(NearbyUuids.SERVICE)).build()),
            ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(),
            callback,
        )
        sweeper = scope.launch {
            while (isActive) {
                delay(5_000)
                val now = System.currentTimeMillis()
                lastHeard.filterValues { now - it > LOST_AFTER_MS }.keys.forEach { address ->
                    lastHeard.remove(address)
                    if (known.remove(address)) onLost(address)
                }
            }
        }
        return true
    }

    fun stop() {
        sweeper?.cancel()
        runCatching { bluetooth(context)?.bluetoothLeScanner?.stopScan(callback) }
        known.forEach(onLost)
        known.clear()
        lastHeard.clear()
    }

    private suspend fun heard(address: String) {
        lastHeard[address] = System.currentTimeMillis()
        if (address in known || !reading.add(address)) return
        try {
            val beacon = GattClient.readBeacon(context, address) ?: return
            known.add(address)
            onFound(address, beacon)
        } finally {
            reading.remove(address)
        }
    }

    companion object {
        private const val LOST_AFTER_MS = 15_000L
    }
}

/** One short GATT connection per request; requests are serialized app-wide. */
@SuppressLint("MissingPermission")
class GattClient private constructor(private val gatt: BluetoothGatt, private val callback: Callback) {
    private class Callback : BluetoothGattCallback() {
        val connected = CompletableDeferred<Boolean>()
        val ready = CompletableDeferred<Boolean>()
        var read: CompletableDeferred<ByteArray?>? = null
        var write: CompletableDeferred<Boolean>? = null

        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> connected.complete(true)
                BluetoothProfile.STATE_DISCONNECTED -> {
                    connected.complete(false)
                    ready.complete(false)
                    read?.complete(null)
                    write?.complete(false)
                }
            }
        }

        override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) {
            if (!gatt.discoverServices()) ready.complete(false)
        }

        override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
            ready.complete(status == BluetoothGatt.GATT_SUCCESS)
        }

        override fun onCharacteristicRead(
            gatt: BluetoothGatt,
            characteristic: BluetoothGattCharacteristic,
            value: ByteArray,
            status: Int,
        ) {
            read?.complete(value.takeIf { status == BluetoothGatt.GATT_SUCCESS })
        }

        @Deprecated("Called before Android 13")
        override fun onCharacteristicRead(
            gatt: BluetoothGatt,
            characteristic: BluetoothGattCharacteristic,
            status: Int,
        ) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
                @Suppress("DEPRECATION")
                read?.complete(characteristic.value.takeIf { status == BluetoothGatt.GATT_SUCCESS })
            }
        }

        override fun onCharacteristicWrite(
            gatt: BluetoothGatt,
            characteristic: BluetoothGattCharacteristic,
            status: Int,
        ) {
            write?.complete(status == BluetoothGatt.GATT_SUCCESS)
        }
    }

    private fun characteristic(uuid: UUID) = gatt.getService(NearbyUuids.SERVICE)?.getCharacteristic(uuid)

    suspend fun read(uuid: UUID): ByteArray? {
        val target = characteristic(uuid) ?: return null
        val result = CompletableDeferred<ByteArray?>().also { callback.read = it }
        if (!gatt.readCharacteristic(target)) return null
        return result.await()
    }

    suspend fun write(uuid: UUID, value: ByteArray): Boolean {
        val target = characteristic(uuid) ?: return false
        val result = CompletableDeferred<Boolean>().also { callback.write = it }
        val type = BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
        val started = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            gatt.writeCharacteristic(target, value, type) == BluetoothGatt.GATT_SUCCESS
        } else {
            @Suppress("DEPRECATION")
            target.value = value
            target.writeType = type
            @Suppress("DEPRECATION")
            gatt.writeCharacteristic(target)
        }
        return started && result.await()
    }

    companion object {
        private val lock = Mutex()

        /** Connects, runs [block], disconnects. Null on any Bluetooth failure or timeout. */
        suspend fun <T> use(context: Context, address: String, block: suspend (GattClient) -> T): T? =
            lock.withLock {
                val adapter = bluetooth(context) ?: return null
                val callback = Callback()
                val gatt = adapter.getRemoteDevice(address)
                    .connectGatt(context, false, callback, BluetoothDevice.TRANSPORT_LE)
                    ?: return null
                try {
                    withTimeoutOrNull(15_000) {
                        if (!callback.connected.await()) return@withTimeoutOrNull null
                        // A large MTU keeps the ~500 byte beacon to one or two reads.
                        if (!gatt.requestMtu(517)) gatt.discoverServices()
                        if (!callback.ready.await()) return@withTimeoutOrNull null
                        block(GattClient(gatt, callback))
                    }
                } catch (_: SecurityException) {
                    null
                } finally {
                    runCatching { gatt.disconnect() }
                    runCatching { gatt.close() }
                }
            }

        suspend fun readBeacon(context: Context, address: String): Beacon? =
            use(context, address) { it.read(NearbyUuids.BEACON) }?.let(Beacon::parse)

        suspend fun readHotspot(context: Context, address: String): HotspotCredentials? =
            use(context, address) { it.read(NearbyUuids.HOTSPOT) }?.let(HotspotCredentials::parse)

        suspend fun write(context: Context, address: String, uuid: UUID, value: ByteArray): Boolean =
            use(context, address) { it.write(uuid, value) } == true
    }
}
