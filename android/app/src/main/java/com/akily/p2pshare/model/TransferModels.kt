package com.akily.p2pshare.model

import android.net.Uri

enum class TransferTab {
    SEND,
    RECEIVE,
    HISTORY,
}

enum class TransferStage {
    IDLE,
    PREPARING,
    WAITING,
    VERIFYING,
    TRANSFERRING,
    COMPLETED,
    FAILED,
}

data class TransferHistoryEntry(
    val timestampMs: Long,
    val direction: String,
    val fileName: String,
    val path: String?,
    val sizeBytes: Long,
    val success: Boolean,
    val detail: String,
)

enum class SendMode { WAIT, TICKET, NEARBY }

enum class ReceiveMode { CONNECT, LISTEN, NEARBY }

/** A nearby sender waiting for this device to accept. */
data class OfferUi(
    val senderName: String,
    val fileName: String,
    val sizeBytes: Long,
    val itemCount: Long,
)

data class TransferUiState(
    val stage: TransferStage = TransferStage.IDLE,
    val statusLine: String = "Ready",
    val ticket: String? = null,
    val qrPayload: String? = null,
    val handshakeCode: String? = null,
    val progressDone: Long = 0,
    val progressTotal: Long = 0,
    val connectionPath: String? = null,
    val latencyMs: Double? = null,
    val completedName: String? = null,
    val completedSize: Long? = null,
    val completedPath: String? = null,
    val completedContentKind: String? = null,
    val completedItemCount: Long = 1,
    val errorMessage: String? = null,
    val offer: OfferUi? = null,
)

data class SendFormState(
    val fileUris: List<Uri> = emptyList(),
    val preparedPaths: List<String> = emptyList(),
    val sendMode: SendMode = SendMode.WAIT,
    val ticketInput: String = "",
    val selectedPeerId: String? = null,
)

data class ReceiveFormState(
    val receiveMode: ReceiveMode = ReceiveMode.CONNECT,
    val targetInput: String = "",
    val outputTreeUri: Uri? = null,
)
