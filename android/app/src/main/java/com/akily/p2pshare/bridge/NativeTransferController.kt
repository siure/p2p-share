package com.akily.p2pshare.bridge

import android.util.Log
import org.json.JSONArray
import org.json.JSONObject

class NativeTransferController private constructor(
    private var handle: Long,
) : TransferEngine {
    override val usingRust: Boolean = true

    @Synchronized
    override fun startSendWait(filePaths: List<String>) {
        if (handle == 0L) return
        RustBindings.nativeStartSendWait(handle, JSONArray(filePaths).toString())
    }

    @Synchronized
    override fun startSendToTicket(filePaths: List<String>, ticket: String) {
        if (handle == 0L) return
        RustBindings.nativeStartSendToTicket(handle, JSONArray(filePaths).toString(), ticket)
    }

    @Synchronized
    override fun startReceiveTarget(target: String, outputDir: String) {
        if (handle == 0L) return
        RustBindings.nativeStartReceiveTarget(handle, target, outputDir)
    }

    @Synchronized
    override fun startReceiveListen(outputDir: String) {
        if (handle == 0L) return
        RustBindings.nativeStartReceiveListen(handle, outputDir)
    }

    @Synchronized
    override fun pollEvent(): BridgeEvent? {
        if (handle == 0L) return null
        val payload = RustBindings.nativePollEvent(handle) ?: return null
        return payload.toBridgeEvent()
    }

    @Synchronized
    override fun cancel() {
        if (handle == 0L) return
        RustBindings.nativeCancel(handle)
    }

    @Synchronized
    override fun close() {
        val ownedHandle = handle
        handle = 0L
        if (ownedHandle != 0L) RustBindings.nativeDestroyController(ownedHandle)
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

        fun createOrNull(context: android.content.Context): NativeTransferController? {
            if (!RustBindings.initialize(context)) {
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
