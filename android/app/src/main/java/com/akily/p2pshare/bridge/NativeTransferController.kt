package com.akily.p2pshare.bridge

import android.util.Log
import org.json.JSONObject

class NativeTransferController private constructor(
    private val handle: Long,
) : TransferEngine {
    override val usingRust: Boolean = true

    override fun startSendWait(filePath: String) {
        RustBindings.nativeStartSendWait(handle, filePath)
    }

    override fun startSendToTicket(filePath: String, ticket: String) {
        RustBindings.nativeStartSendToTicket(handle, filePath, ticket)
    }

    override fun startReceiveTarget(target: String, outputDir: String) {
        RustBindings.nativeStartReceiveTarget(handle, target, outputDir)
    }

    override fun startReceiveListen(outputDir: String) {
        RustBindings.nativeStartReceiveListen(handle, outputDir)
    }

    override fun pollEvent(): BridgeEvent? {
        val payload = RustBindings.nativePollEvent(handle) ?: return null
        return payload.toBridgeEvent()
    }

    override fun cancel() {
        RustBindings.nativeCancel(handle)
    }

    private fun String.toBridgeEvent(): BridgeEvent {
        val json = JSONObject(this)
        return BridgeEvent(
            kind = json.optString("kind"),
            message = json.optString("message", null),
            value = json.optString("value", null),
            done = if (json.has("done") && !json.isNull("done")) json.optLong("done") else null,
            total = if (json.has("total") && !json.isNull("total")) json.optLong("total") else null,
            fileName = json.optString("file_name", null),
            sizeBytes = if (json.has("size_bytes") && !json.isNull("size_bytes")) json.optLong("size_bytes") else null,
            savedPath = json.optString("saved_path", null),
            latencyMs = if (json.has("latency_ms") && !json.isNull("latency_ms")) json.optDouble("latency_ms") else null,
        )
    }

    companion object {
        private const val TAG = "P2PShareNative"

        fun createOrNull(): NativeTransferController? {
            if (!RustBindings.loaded) {
                Log.w(TAG, "Native library load failed: ${RustBindings.loadError}")
                return null
            }

            val handle = runCatching { RustBindings.nativeCreateController() }.getOrElse { err ->
                Log.e(TAG, "nativeCreateController failed", err)
                return null
            }

            return NativeTransferController(handle)
        }
    }
}
