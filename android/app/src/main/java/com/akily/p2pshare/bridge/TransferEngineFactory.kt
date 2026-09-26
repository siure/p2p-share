package com.akily.p2pshare.bridge

object TransferEngineFactory {
    fun create(context: android.content.Context): TransferEngine = NativeTransferController.createOrNull(context) ?: DemoTransferEngine()
}
