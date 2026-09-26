package com.akily.p2pshare.bridge

object RustBindings {
    private val loadResult = runCatching {
        System.loadLibrary("p2p_share_android_bindings")
    }
    val loaded: Boolean = loadResult.isSuccess
    val loadError: String? = loadResult.exceptionOrNull()?.message

    private var initialized = false

    @Synchronized
    fun initialize(context: android.content.Context): Boolean {
        if (!loaded) return false
        if (!initialized) initialized = nativeInitialize(context.applicationContext)
        return initialized
    }

    @JvmStatic
    private external fun nativeInitialize(context: android.content.Context): Boolean

    @JvmStatic
    external fun nativeCreateController(): Long

    @JvmStatic
    external fun nativeDestroyController(handle: Long)

    @JvmStatic
    external fun nativeStartSendWait(handle: Long, filePathsJson: String)

    @JvmStatic
    external fun nativeStartSendToTicket(handle: Long, filePathsJson: String, ticket: String)

    @JvmStatic
    external fun nativeStartReceiveTarget(handle: Long, target: String, outputDir: String)

    @JvmStatic
    external fun nativeStartReceiveListen(handle: Long, outputDir: String)

    @JvmStatic
    external fun nativePollEvent(handle: Long): String?

    @JvmStatic
    external fun nativeCancel(handle: Long)
}
