package com.akily.p2pshare.bridge

import android.util.Log
import org.json.JSONArray
import org.json.JSONObject

class NativeTransferController private constructor(
    private val handle: Long,
) : TransferEngine {
    override val usingRust: Boolean = true

    override fun startSendWait(filePaths: List<String>) {
        RustBindings.nativeStartSendWait(handle, JSONArray(filePaths).toString())
    }

    override fun startSendToTicket(filePaths: List<String>, ticket: String) {
        RustBindings.nativeStartSendToTicket(handle, JSONArray(filePaths).toString(), ticket)
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
            message = json.optNullableString("message"),
            value = json.optNullableString("value"),
            done = if (json.has("done") && !json.isNull("done")) json.optLong("done") else null,
            total = if (json.has("total") && !json.isNull("total")) json.optLong("total") else null,
            fileName = json.optNullableString("file_name"),
            sizeBytes = if (json.has("size_bytes") && !json.isNull("size_bytes")) json.optLong("size_bytes") else null,
            savedPath = json.optNullableString("saved_path"),
            latencyMs = if (json.has("latency_ms") && !json.isNull("latency_ms")) json.optDouble("latency_ms") else null,
            contentKind = json.optNullableString("content_kind"),
            itemCount = if (json.has("item_count") && !json.isNull("item_count")) json.optLong("item_count") else null,
        )
    }

    private fun JSONObject.optNullableString(key: String): String? =
        if (has(key) && !isNull(key)) optString(key) else null

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
