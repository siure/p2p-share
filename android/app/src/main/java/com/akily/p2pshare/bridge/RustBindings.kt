package com.akily.p2pshare.bridge

object RustBindings {
    private val loadResult = runCatching {
        System.loadLibrary("p2p_share_android_bindings")
    }
    val loaded: Boolean = loadResult.isSuccess
    val loadError: String? = loadResult.exceptionOrNull()?.message

    @JvmStatic
    external fun nativeCreateController(): Long

    @JvmStatic
    external fun nativeStartSendWait(handle: Long, filePath: String)

    @JvmStatic
    external fun nativeStartSendToTicket(handle: Long, filePath: String, ticket: String)

    @JvmStatic
    external fun nativeStartReceiveTarget(handle: Long, target: String, outputDir: String)

    @JvmStatic
    external fun nativeStartReceiveListen(handle: Long, outputDir: String)

    @JvmStatic
    external fun nativePollEvent(handle: Long): String?

    @JvmStatic
    external fun nativeCancel(handle: Long)
}
