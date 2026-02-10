package com.akily.p2pshare.bridge

object TransferEngineFactory {
    fun create(): TransferEngine = NativeTransferController.createOrNull() ?: DemoTransferEngine()
}
