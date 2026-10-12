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
import com.akily.p2pshare.bridge.BridgeEvent
import com.akily.p2pshare.bridge.TransferEngine
import com.akily.p2pshare.bridge.TransferEngineFactory
import com.akily.p2pshare.model.OfferUi
import com.akily.p2pshare.model.ReceiveFormState
import com.akily.p2pshare.model.ReceiveMode
import com.akily.p2pshare.model.SendFormState
import com.akily.p2pshare.model.SendMode
import com.akily.p2pshare.model.TransferHistoryEntry
import com.akily.p2pshare.model.TransferStage
import com.akily.p2pshare.model.TransferTab
import com.akily.p2pshare.model.TransferUiState
import com.akily.p2pshare.nearby.NearbyManager
import com.akily.p2pshare.storage.FilePrep
import com.akily.p2pshare.storage.PrefsStore
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

class TransferViewModel(app: Application) : AndroidViewModel(app) {
    private val engine: TransferEngine = TransferEngineFactory.create(app)
    private val prefs = PrefsStore(app)
    val nearby = NearbyManager(app, viewModelScope, engine)
    private var reachJob: Job? = null
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

    var selectedFilesLabel by mutableStateOf("")
        private set
    var outputFolderLabel by mutableStateOf("Not selected")
        private set
    private var preparationJob: Job? = null
    private val ownedPreparedFiles = mutableSetOf<File>()
    private var activeSendPaths = emptySet<String>()
    private var pollJob: Job? = null

    init { refreshOutputFolderLabel(receiveForm.outputTreeUri) }

    private var activeDirection: String = "send"
    private var pollGeneration: Long = 0

    fun selectTab(tab: TransferTab) {
        selectedTab = tab
        updateBrowsing()
    }

    /** Callers request the nearby permissions before choosing [SendMode.NEARBY]. */
    fun setSendMode(mode: SendMode) {
        sendForm = sendForm.copy(sendMode = mode)
        updateBrowsing()
    }

    fun selectPeer(id: String) {
        sendForm = sendForm.copy(selectedPeerId = id)
    }

    /** Browse only while the nearby send list is on screen. */
    private fun updateBrowsing() {
        if (selectedTab == TransferTab.SEND && sendForm.sendMode == SendMode.NEARBY) {
            nearby.startBrowsing()
        } else {
            nearby.stopBrowsing()
        }
    }

    fun answerOffer(accept: Boolean) {
        engine.answerOffer(accept)
        receiveUi = receiveUi.copy(offer = null)
    }

    fun setTicketInput(ticket: String) {
        sendForm = sendForm.copy(ticketInput = ticket)
    }

    fun setReceiveMode(mode: ReceiveMode) {
        receiveForm = receiveForm.copy(receiveMode = mode)
    }

    fun setTargetInput(target: String) {
        receiveForm = receiveForm.copy(targetInput = target)
    }

    fun setFileUris(uris: List<Uri>) {
        sendForm = sendForm.copy(fileUris = uris, preparedPaths = emptyList())
    }

    fun setOutputTree(uri: Uri?) {
        receiveForm = receiveForm.copy(outputTreeUri = uri)
        prefs.saveOutputTree(uri)
        refreshOutputFolderLabel(uri)
    }

    fun prepareSendFiles(uris: List<Uri>) {
        preparationJob?.cancel()
        releaseUnusedPreparedFiles(emptySet())
        sendForm = sendForm.copy(fileUris = uris, preparedPaths = emptyList())
        selectedFilesLabel = if (uris.isEmpty()) "" else "${uris.size} selected"
        if (uris.isEmpty()) {
            sendUi = sendUi.copy(stage = TransferStage.IDLE, statusLine = readyStatus, errorMessage = null)
            return
        }
        sendUi = sendUi.copy(stage = TransferStage.PREPARING, statusLine = "Preparing files...")
        preparationJob = viewModelScope.launch {
            var copied: List<File>? = null
            try {
                val label = withContext(Dispatchers.IO) { describeFileUris(uris) }
                if (sendForm.fileUris != uris) return@launch
                selectedFilesLabel = label
                withContext(Dispatchers.IO) {
                    // Assign inside IO so cancellation on dispatch back still cleans up.
                    copied = FilePrep.copyUrisToCacheFiles(getApplication(), uris)
                }
                if (sendForm.fileUris != uris) return@launch
                val files = requireNotNull(copied)
                ownedPreparedFiles.addAll(files)
                copied = null
                sendForm = sendForm.copy(preparedPaths = files.map(File::getAbsolutePath))
                sendUi = sendUi.copy(stage = TransferStage.IDLE,
                    statusLine = if (files.size == 1) "File prepared" else "${files.size} files prepared",
                    errorMessage = null)
            } catch (err: CancellationException) {
                throw err
            } catch (err: Exception) {
                if (sendForm.fileUris == uris) {
                    sendUi = sendUi.copy(stage = TransferStage.FAILED,
                        statusLine = "Failed to prepare selected files.", errorMessage = err.message)
                }
            } finally {
                val unused = copied
                if (unused != null) withContext(NonCancellable + Dispatchers.IO) {
                    unused.forEach { runCatching { FilePrep.deletePreparedFile(getApplication(), it) } }
                }
            }
        }
    }

    private fun releaseUnusedPreparedFiles(keep: Set<String> = sendForm.preparedPaths.toSet()) {
        val unused = ownedPreparedFiles.filter { it.absolutePath !in keep && it.absolutePath !in activeSendPaths }
        ownedPreparedFiles.removeAll(unused.toSet())
        val app = getApplication<Application>()
        CoroutineScope(Dispatchers.IO).launch {
            unused.forEach { runCatching { FilePrep.deletePreparedFile(app, it) } }
        }
    }

    private fun finishSendOwnership() {
        activeSendPaths = emptySet()
        releaseUnusedPreparedFiles()
    }

    override fun onCleared() {
        preparationJob?.cancel()
        reachJob?.cancel()
        stopPolling()
        nearby.close()
        engine.close()
        activeSendPaths = emptySet()
        releaseUnusedPreparedFiles(emptySet())
        super.onCleared()
    }

    private fun refreshOutputFolderLabel(uri: Uri?) {
        viewModelScope.launch {
            val label = withContext(Dispatchers.IO) { describeUri(uri) }
            if (receiveForm.outputTreeUri == uri) outputFolderLabel = label
        }
    }

    fun startSend() {
        val paths = sendForm.preparedPaths
        if (paths.isEmpty()) {
            sendUi = sendUi.copy(
                stage = TransferStage.FAILED,
                statusLine = "Select files first.",
            )
            return
        }

        val invalidPath = paths.firstOrNull { path ->
            val prepared = File(path)
            !prepared.exists() || !prepared.isFile
        }
        if (invalidPath != null) {
            sendUi = sendUi.copy(
                stage = TransferStage.FAILED,
                statusLine = "Selected files are unavailable.",
                errorMessage = "One or more prepared files are unavailable. Pick the files again.",
            )
            sendForm = sendForm.copy(preparedPaths = emptyList())
            return
        }

        val peer = nearby.peers.find { it.id == sendForm.selectedPeerId }
        if (sendForm.sendMode == SendMode.NEARBY && peer == null) {
            sendUi = sendUi.copy(stage = TransferStage.FAILED, statusLine = "Choose a nearby device.")
            return
        }

        activeSendPaths = paths.toSet()
        resetSendUi()
        activeDirection = "send"
        sendUi = sendUi.copy(stage = TransferStage.PREPARING, statusLine = "Starting send...")

        when (sendForm.sendMode) {
            SendMode.TICKET -> engine.startSendToTicket(paths, sendForm.ticketInput.trim())
            SendMode.WAIT -> engine.startSendWait(paths)
            SendMode.NEARBY -> {
                reachJob = viewModelScope.launch {
                    val ticket = try {
                        nearby.reach(requireNotNull(peer)) { sendUi = sendUi.copy(statusLine = it) }
                    } catch (err: CancellationException) {
                        throw err
                    } catch (err: Exception) {
                        nearby.releaseNetwork()
                        finishSendOwnership()
                        sendUi = sendUi.copy(stage = TransferStage.FAILED, statusLine = "Transfer failed",
                            errorMessage = err.message)
                        return@launch
                    }
                    engine.startSendToTicket(paths, ticket)
                    startPolling()
                }
                return
            }
        }

        startPolling()
    }

    fun startReceive() {
        val outputDir = File(getApplication<Application>().filesDir, "incoming")
        outputDir.mkdirs()

        resetReceiveUi()
        activeDirection = "receive"
        receiveUi = receiveUi.copy(stage = TransferStage.PREPARING, statusLine = "Starting receive...")

        when (receiveForm.receiveMode) {
            ReceiveMode.CONNECT -> engine.startReceiveTarget(receiveForm.targetInput.trim(), outputDir.absolutePath)
            ReceiveMode.LISTEN -> engine.startReceiveListen(outputDir.absolutePath)
            ReceiveMode.NEARBY -> {
                engine.startReceiveNearby(outputDir.absolutePath)
                if (!nearby.startAdvertising { receiveUi = receiveUi.copy(statusLine = it) }) {
                    receiveUi = receiveUi.copy(statusLine = "Bluetooth unavailable: visible on this network only.")
                }
            }
        }

        startPolling()
    }

    fun cancelTransfer() {
        if (activeDirection == "send") preparationJob?.cancel()
        reachJob?.cancel()
        engine.cancel()
        endNearbySession()
        finishSendOwnership()
        stopPolling()
        updateActiveUi {
            it.copy(
                stage = TransferStage.IDLE,
                statusLine = "Canceled",
                ticket = null,
                qrPayload = null,
                progressDone = 0,
                progressTotal = 0,
                offer = null,
            )
        }
    }

    /** Stop advertising or leave a hotspot joined for this transfer. */
    private fun endNearbySession() {
        if (activeDirection == "receive") nearby.stopAdvertising() else nearby.releaseNetwork()
    }

    fun isBusy(): Boolean = listOf(sendUi, receiveUi).any {
        it.stage in listOf(TransferStage.PREPARING, TransferStage.WAITING,
            TransferStage.VERIFYING, TransferStage.TRANSFERRING)
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
            var finished = false
            while (isActive && !finished) {
                // Bound each UI turn even when native events arrive continuously.
                for (index in 0 until 32) {
                    val event = engine.pollEvent() ?: break
                    if (applyEvent(event)) {
                        finished = true
                        break
                    }
                }
                if (!finished) delay(16)
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

    private suspend fun applyEvent(event: BridgeEvent): Boolean {
        val current = currentUi()
        when (event.kind) {
            "status" -> {
                val stage = when {
                    event.message?.contains("waiting", ignoreCase = true) == true -> TransferStage.WAITING
                    event.message?.contains("connected", ignoreCase = true) == true -> TransferStage.VERIFYING
                    else -> current.stage
                }
                // The engine declines on its own when an offer goes unanswered.
                val offer = current.offer.takeUnless { event.message?.startsWith("Declined") == true }
                setCurrentUi(current.copy(statusLine = event.message ?: current.statusLine, stage = stage, offer = offer))
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

            "beacon" -> {
                event.value?.let(nearby::updateBeacon)
                return false
            }

            "offer" -> {
                val offer = OfferUi(
                    senderName = event.message ?: "A nearby device",
                    fileName = event.fileName ?: "files",
                    sizeBytes = event.sizeBytes ?: 0,
                    itemCount = event.itemCount ?: 1,
                )
                setCurrentUi(current.copy(offer = offer))
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
                if (activeDirection == "send") finishSendOwnership()
                endNearbySession()
                var finalPath = event.savedPath
                val outputTree = receiveForm.outputTreeUri
                if (activeDirection == "receive" && finalPath != null && outputTree != null) {
                    setCurrentUi(current.copy(statusLine = "Saving files..."))
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
                        completedContentKind = event.contentKind,
                        completedItemCount = event.itemCount ?: 1,
                        errorMessage = null,
                    )
                )
                appendHistory(success = true, detail = "completed")
                return true
            }

            "error" -> {
                if (activeDirection == "send") finishSendOwnership()
                endNearbySession()
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
            FilePrep.copyReceivedEntryToTree(getApplication(), sourcePath, tree, fileName)
        }.getOrNull()
        if (uri != null) runCatching { FilePrep.deleteExportedStaging(getApplication(), sourcePath) }
        return uri?.toString() ?: sourcePath
    }

    private fun appendHistory(success: Boolean, detail: String) {
        val ui = currentUi()
        val path = ui.completedPath ?: sendForm.preparedPaths.firstOrNull() ?: receiveForm.outputTreeUri?.toString()
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

    fun describeFileUris(uris: List<Uri>): String {
        if (uris.isEmpty()) return ""
        val names = uris.map(::displayNameForUri)
        if (names.size == 1) {
            return names.first()
        }
        val preview = names.take(3).joinToString(", ")
        val suffix = if (names.size > 3) ", +${names.size - 3} more" else ""
        return "${names.size} files: $preview$suffix"
    }

    private fun displayNameForUri(uri: Uri): String {
        val doc = DocumentFile.fromSingleUri(getApplication(), uri)
        val name = doc?.name?.trim().orEmpty()
        if (name.isNotEmpty()) return name

        return uri.lastPathSegment
            ?.substringAfterLast('/')
            ?.substringAfterLast(':')
            ?.trim()
            .orEmpty()
    }
}
