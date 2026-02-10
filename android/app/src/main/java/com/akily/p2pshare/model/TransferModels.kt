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
    val errorMessage: String? = null,
)

data class SendFormState(
    val fileUri: Uri? = null,
    val preparedPath: String? = null,
    val sendToTicketMode: Boolean = false,
    val ticketInput: String = "",
)

data class ReceiveFormState(
    val connectMode: Boolean = true,
    val targetInput: String = "",
    val outputTreeUri: Uri? = null,
)
