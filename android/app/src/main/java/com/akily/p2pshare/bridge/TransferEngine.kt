package com.akily.p2pshare.bridge

import java.util.concurrent.ConcurrentLinkedQueue
import kotlin.concurrent.thread

data class BridgeEvent(
    val kind: String,
    val message: String? = null,
    val value: String? = null,
    val done: Long? = null,
    val total: Long? = null,
    val fileName: String? = null,
    val sizeBytes: Long? = null,
    val savedPath: String? = null,
    val latencyMs: Double? = null,
    val contentKind: String? = null,
    val itemCount: Long? = null,
)

/** A receiver found over mDNS by the native engine; `ticket` is null when lost. */
data class NearbyRecord(val peerId: String, val name: String?, val ticket: String?)

interface TransferEngine {
    val usingRust: Boolean

    fun startSendWait(filePaths: List<String>)
    fun startSendToTicket(filePaths: List<String>, ticket: String)
    fun startReceiveTarget(target: String, outputDir: String)
    fun startReceiveListen(outputDir: String)
    /** Listen while advertised over mDNS; emits `beacon` and `offer` events. */
    fun startReceiveNearby(outputDir: String)
    fun answerOffer(accept: Boolean): Boolean
    fun setDeviceName(name: String)
    fun hasRelay(): Boolean
    /** Recreate the endpoint after the process switched networks. Cancels any transfer. */
    fun rebind()
    fun startBrowse()
    fun stopBrowse()
    fun pollNearby(): NearbyRecord?
    fun pollEvent(): BridgeEvent?
    fun cancel()
    fun close() = cancel()
}

class DemoTransferEngine : TransferEngine {
    override val usingRust: Boolean = false
    private val queue = ConcurrentLinkedQueue<BridgeEvent>()
    @Volatile
    private var canceled = false

    override fun startSendWait(filePaths: List<String>) {
        runDemo("send", withTicket = true)
    }

    override fun startSendToTicket(filePaths: List<String>, ticket: String) {
        runDemo("send", withTicket = false)
    }

    override fun startReceiveTarget(target: String, outputDir: String) {
        runDemo("receive", withTicket = false, outputDir = outputDir)
    }

    override fun startReceiveListen(outputDir: String) {
        runDemo("receive", withTicket = true, outputDir = outputDir)
    }

    override fun startReceiveNearby(outputDir: String) {
        runDemo("receive", withTicket = true, outputDir = outputDir)
    }

    override fun answerOffer(accept: Boolean): Boolean = false
    override fun setDeviceName(name: String) = Unit
    override fun hasRelay(): Boolean = false
    override fun rebind() = Unit
    override fun startBrowse() = Unit
    override fun stopBrowse() = Unit
    override fun pollNearby(): NearbyRecord? = null

    override fun pollEvent(): BridgeEvent? = queue.poll()

    override fun cancel() {
        canceled = true
        queue.add(BridgeEvent(kind = "status", message = "Transfer canceled by user."))
    }

    private fun runDemo(direction: String, withTicket: Boolean, outputDir: String? = null) {
        canceled = false
        queue.clear()
        queue.add(BridgeEvent(kind = "status", message = "Demo mode: native Rust library not loaded."))
        queue.add(BridgeEvent(kind = "status", message = "Preparing transfer..."))
        if (withTicket) {
            val ticket = "p2psh:demo-ticket-${System.currentTimeMillis()}"
            queue.add(BridgeEvent(kind = "ticket", value = ticket))
            queue.add(BridgeEvent(kind = "qr_payload", value = ticket))
            queue.add(BridgeEvent(kind = "status", message = "Waiting for peer to connect..."))
        }
        queue.add(BridgeEvent(kind = "handshake_code", value = "1a2b-3c4d"))
        queue.add(BridgeEvent(kind = "connection_path", value = "relay", message = "wss://relay.iroh.network", latencyMs = 32.8))

        thread(name = "demo-transfer", isDaemon = true) {
            val total = 5_000_000L
            var done = 0L
            while (done < total && !canceled) {
                Thread.sleep(130)
                done += 240_000L
                if (done > total) done = total
                queue.add(BridgeEvent(kind = "progress", done = done, total = total))
            }
            if (canceled) return@thread

            val fileName = if (direction == "send") "sample.bin" else "incoming.bin"
            val savedPath = if (direction == "receive") "$outputDir/$fileName" else null
            queue.add(
                BridgeEvent(
                    kind = "completed",
                    fileName = fileName,
                    sizeBytes = total,
                    savedPath = savedPath,
                    contentKind = "file",
                    itemCount = 1,
                )
            )
            queue.add(BridgeEvent(kind = "status", message = "Checksum verified (blake3)."))
        }
    }
}
