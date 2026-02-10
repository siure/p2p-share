package com.akily.p2pshare.ui

import android.app.Application
import android.content.ContentResolver
import android.net.Uri
import androidx.documentfile.provider.DocumentFile
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.akily.p2pshare.bridge.TransferEngine
import com.akily.p2pshare.bridge.TransferEngineFactory
import com.akily.p2pshare.model.ReceiveFormState
import com.akily.p2pshare.model.SendFormState
import com.akily.p2pshare.model.TransferHistoryEntry
import com.akily.p2pshare.model.TransferStage
import com.akily.p2pshare.model.TransferTab
import com.akily.p2pshare.model.TransferUiState
import com.akily.p2pshare.storage.FilePrep
import com.akily.p2pshare.storage.PrefsStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

class TransferViewModel(app: Application) : AndroidViewModel(app) {
    private val engine: TransferEngine = TransferEngineFactory.create()
    private val prefs = PrefsStore(app)
    private val readyStatus = "Ready"

    var selectedTab by mutableStateOf(TransferTab.SEND)
        private set

    var sendUi by mutableStateOf(TransferUiState(statusLine = readyStatus))
        private set

    var receiveUi by mutableStateOf(TransferUiState(statusLine = readyStatus))
        private set

    var sendForm by mutableStateOf(SendFormState())
        private set

    var receiveForm by mutableStateOf(ReceiveFormState(outputTreeUri = prefs.loadOutputTree()))
        private set

    var history by mutableStateOf<List<TransferHistoryEntry>>(emptyList())
        private set

    private var pollJob: Job? = null
    private var activeDirection: String = "send"
    private var pollGeneration: Long = 0

    fun selectTab(tab: TransferTab) {
        selectedTab = tab
    }

    fun setSendMode(sendToTicket: Boolean) {
        sendForm = sendForm.copy(sendToTicketMode = sendToTicket)
    }

    fun setTicketInput(ticket: String) {
        sendForm = sendForm.copy(ticketInput = ticket)
    }

    fun setReceiveMode(connectMode: Boolean) {
        receiveForm = receiveForm.copy(connectMode = connectMode)
    }

    fun setTargetInput(target: String) {
        receiveForm = receiveForm.copy(targetInput = target)
    }

    fun setFileUri(uri: Uri?) {
        sendForm = sendForm.copy(fileUri = uri)
    }

    fun setOutputTree(uri: Uri?) {
        receiveForm = receiveForm.copy(outputTreeUri = uri)
        prefs.saveOutputTree(uri)
    }

    fun prepareSendFile(uri: Uri?) {
        if (uri == null) {
            sendForm = sendForm.copy(preparedPath = null)
            return
        }
        sendForm = sendForm.copy(fileUri = uri, preparedPath = null)
        sendUi = sendUi.copy(stage = TransferStage.PREPARING, statusLine = "Preparing file...")

        viewModelScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    FilePrep.copyUriToCacheFile(getApplication(), uri)
                }
            }.onSuccess { file ->
                if (sendForm.fileUri != uri) return@onSuccess
                sendForm = sendForm.copy(fileUri = uri, preparedPath = file.absolutePath)
                sendUi = sendUi.copy(
                    stage = TransferStage.IDLE,
                    statusLine = "File prepared: ${file.name}",
                    errorMessage = null,
                )
            }.onFailure { err ->
                if (sendForm.fileUri != uri) return@onFailure
                sendForm = sendForm.copy(preparedPath = null)
                sendUi = sendUi.copy(
                    stage = TransferStage.FAILED,
                    statusLine = "Failed to prepare selected file.",
                    errorMessage = err.message,
                )
            }
        }
    }

    fun startSend() {
        val path = sendForm.preparedPath
        if (path.isNullOrBlank()) {
            sendUi = sendUi.copy(
                stage = TransferStage.FAILED,
                statusLine = "Select a file first.",
            )
            return
        }
        val prepared = File(path)
        if (!prepared.exists() || !prepared.isFile || prepared.length() == 0L) {
            sendUi = sendUi.copy(
                stage = TransferStage.FAILED,
                statusLine = "Selected file is unavailable.",
                errorMessage = "Prepared file is missing or empty. Pick the file again.",
            )
            sendForm = sendForm.copy(preparedPath = null)
            return
        }

        resetSendUi()
        activeDirection = "send"
        sendUi = sendUi.copy(stage = TransferStage.PREPARING, statusLine = "Starting send...")

        if (sendForm.sendToTicketMode) {
            engine.startSendToTicket(path, sendForm.ticketInput.trim())
        } else {
            engine.startSendWait(path)
        }

        startPolling()
    }

    fun startReceive() {
        val outputDir = File(getApplication<Application>().filesDir, "incoming")
        outputDir.mkdirs()

        resetReceiveUi()
        activeDirection = "receive"
        receiveUi = receiveUi.copy(stage = TransferStage.PREPARING, statusLine = "Starting receive...")

        if (receiveForm.connectMode) {
            engine.startReceiveTarget(receiveForm.targetInput.trim(), outputDir.absolutePath)
        } else {
            engine.startReceiveListen(outputDir.absolutePath)
        }

        startPolling()
    }

    fun cancelTransfer() {
        engine.cancel()
        stopPolling()
        updateActiveUi {
            it.copy(
            stage = TransferStage.IDLE,
            statusLine = "Canceled",
            ticket = null,
            qrPayload = null,
            progressDone = 0,
            progressTotal = 0,
            )
        }
    }

    fun canCancelSend(): Boolean {
        if (activeDirection != "send") return false
        return when (sendUi.stage) {
            TransferStage.PREPARING,
            TransferStage.WAITING,
            TransferStage.VERIFYING,
            TransferStage.TRANSFERRING,
            -> true

            TransferStage.IDLE,
            TransferStage.COMPLETED,
            TransferStage.FAILED,
            -> false
        }
    }

    fun canCancelReceive(): Boolean {
        if (activeDirection != "receive") return false
        return when (receiveUi.stage) {
            TransferStage.PREPARING,
            TransferStage.WAITING,
            TransferStage.VERIFYING,
            TransferStage.TRANSFERRING,
            -> true

            TransferStage.IDLE,
            TransferStage.COMPLETED,
            TransferStage.FAILED,
            -> false
        }
    }

    private fun startPolling() {
        stopPolling()
        pollGeneration += 1
        val generation = pollGeneration
        pollJob = viewModelScope.launch {
            while (isActive) {
                val event = engine.pollEvent()
                if (event != null) {
                    val shouldStop = applyEvent(event)
                    if (shouldStop) {
                        break
                    }
                } else {
                    delay(80)
                }
            }
            if (pollGeneration == generation) {
                pollJob = null
            }
        }
    }

    private fun stopPolling() {
        pollJob?.cancel()
        pollJob = null
    }

    private suspend fun applyEvent(event: com.akily.p2pshare.bridge.BridgeEvent): Boolean {
        val current = currentUi()
        when (event.kind) {
            "status" -> {
                val stage = when {
                    event.message?.contains("waiting", ignoreCase = true) == true -> TransferStage.WAITING
                    event.message?.contains("connected", ignoreCase = true) == true -> TransferStage.VERIFYING
                    else -> current.stage
                }
                setCurrentUi(current.copy(statusLine = event.message ?: current.statusLine, stage = stage))
                return false
            }

            "ticket" -> {
                setCurrentUi(current.copy(ticket = event.value))
                return false
            }

            "qr_payload" -> {
                setCurrentUi(current.copy(qrPayload = event.value))
                return false
            }

            "handshake_code" -> {
                setCurrentUi(current.copy(handshakeCode = event.value, stage = TransferStage.VERIFYING))
                return false
            }

            "progress" -> {
                setCurrentUi(
                    current.copy(
                        progressDone = event.done ?: 0,
                        progressTotal = event.total ?: 0,
                        stage = TransferStage.TRANSFERRING,
                    )
                )
                return false
            }

            "connection_path" -> {
                setCurrentUi(
                    current.copy(
                        connectionPath = listOfNotNull(event.value, event.message).joinToString(" • "),
                        latencyMs = event.latencyMs,
                    )
                )
                return false
            }

            "completed" -> {
                var finalPath = event.savedPath
                val outputTree = receiveForm.outputTreeUri
                if (activeDirection == "receive" && finalPath != null && outputTree != null) {
                    finalPath = withContext(Dispatchers.IO) {
                        copyToOutputTree(finalPath, event.fileName ?: "received.bin", outputTree)
                    }
                }

                setCurrentUi(
                    current.copy(
                        stage = TransferStage.COMPLETED,
                        statusLine = "Transfer completed",
                        completedName = event.fileName,
                        completedSize = event.sizeBytes,
                        completedPath = finalPath,
                        errorMessage = null,
                    )
                )
                appendHistory(success = true, detail = "completed")
                return true
            }

            "error" -> {
                setCurrentUi(
                    current.copy(
                        stage = TransferStage.FAILED,
                        statusLine = "Transfer failed",
                        errorMessage = event.message ?: "Unknown error",
                    )
                )
                appendHistory(success = false, detail = event.message ?: "error")
                return true
            }

            else -> return false
        }
    }

    private fun copyToOutputTree(sourcePath: String, fileName: String, tree: Uri): String? {
        val uri = runCatching {
            FilePrep.copyReceivedFileToTree(getApplication(), sourcePath, tree, fileName)
        }.getOrNull()
        return uri?.toString() ?: sourcePath
    }

    private fun appendHistory(success: Boolean, detail: String) {
        val ui = currentUi()
        val path = ui.completedPath ?: sendForm.preparedPath ?: receiveForm.outputTreeUri?.toString()
        val inferredName = path
            ?.replace('\\', '/')
            ?.substringAfterLast('/')
            ?.takeIf { it.isNotBlank() }
        val entry = TransferHistoryEntry(
            timestampMs = System.currentTimeMillis(),
            direction = activeDirection,
            fileName = ui.completedName ?: inferredName ?: "unknown",
            path = path,
            sizeBytes = ui.completedSize ?: 0,
            success = success,
            detail = detail,
        )
        history = (listOf(entry) + history).take(100)
    }

    private fun resetSendUi() {
        sendUi = TransferUiState(
            stage = TransferStage.IDLE,
            statusLine = readyStatus,
        )
    }

    private fun resetReceiveUi() {
        receiveUi = TransferUiState(
            stage = TransferStage.IDLE,
            statusLine = readyStatus,
        )
    }

    private fun currentUi(): TransferUiState =
        if (activeDirection == "receive") receiveUi else sendUi

    private fun setCurrentUi(value: TransferUiState) {
        if (activeDirection == "receive") {
            receiveUi = value
        } else {
            sendUi = value
        }
    }

    private fun updateActiveUi(transform: (TransferUiState) -> TransferUiState) {
        setCurrentUi(transform(currentUi()))
    }

    fun rememberOutputTreePermission(contentResolver: ContentResolver, uri: Uri) {
        runCatching {
            contentResolver.takePersistableUriPermission(
                uri,
                android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
            )
        }
        setOutputTree(uri)
    }

    fun describeUri(uri: Uri?): String {
        if (uri == null) return "Not selected"
        val doc = DocumentFile.fromTreeUri(getApplication(), uri)
        return doc?.name ?: uri.toString()
    }

    fun describeFileUri(uri: Uri?): String {
        if (uri == null) return ""
        val doc = DocumentFile.fromSingleUri(getApplication(), uri)
        val name = doc?.name?.trim().orEmpty()
        if (name.isNotEmpty()) return name

        val fallback = uri.lastPathSegment
            ?.substringAfterLast('/')
            ?.substringAfterLast(':')
            ?.trim()
            .orEmpty()
        return fallback
    }
}
