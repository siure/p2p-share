package com.akily.p2pshare.ui

import android.graphics.Bitmap
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Cancel
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.FolderOpen
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.akily.p2pshare.model.TransferHistoryEntry
import com.akily.p2pshare.model.TransferStage
import com.akily.p2pshare.model.TransferTab
import com.akily.p2pshare.model.TransferUiState
import com.akily.p2pshare.ui.theme.ElectricBlue
import com.akily.p2pshare.ui.theme.GlowBlue
import com.akily.p2pshare.ui.theme.GlowCyan
import com.akily.p2pshare.ui.theme.NeonCyan
import com.akily.p2pshare.ui.theme.NightBase
import com.akily.p2pshare.ui.theme.NightLayer
import com.akily.p2pshare.ui.theme.NightSurface
import com.akily.p2pshare.ui.theme.NightText
import com.akily.p2pshare.ui.theme.SignalGreen
import com.akily.p2pshare.ui.theme.SignalRed
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

private val AccentViolet = Color(0xFFFF8F74)
private val BottomGlowMauve = Color(0xFFFF8F74)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun P2PShareApp(viewModel: TransferViewModel) {
    val sendUi = viewModel.sendUi
    val receiveUi = viewModel.receiveUi
    val send = viewModel.sendForm
    val receive = viewModel.receiveForm
    val selectedTab = viewModel.selectedTab
    val tabs = TransferTab.entries
    val context = LocalContext.current
    var scanDestination by remember { mutableStateOf("send") }

    val pagerState = rememberPagerState(
        initialPage = pageForTab(selectedTab),
        pageCount = { tabs.size },
    )
    val filePicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        viewModel.setFileUri(uri)
        viewModel.prepareSendFile(uri)
    }

    val outputPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        if (uri != null) {
            viewModel.rememberOutputTreePermission(context.contentResolver, uri)
        }
    }

    val qrScanner = rememberLauncherForActivityResult(ScanContract()) { result ->
        val scanned = result.contents?.trim().orEmpty()
        if (scanned.isNotEmpty()) {
            if (scanDestination == "receive") {
                viewModel.setReceiveMode(true)
                viewModel.setTargetInput(scanned)
                viewModel.selectTab(TransferTab.RECEIVE)
            } else {
                viewModel.setSendMode(true)
                viewModel.setTicketInput(scanned)
                viewModel.selectTab(TransferTab.SEND)
            }
        }
    }

    LaunchedEffect(selectedTab) {
        val targetPage = pageForTab(selectedTab)
        if (pagerState.currentPage != targetPage) {
            pagerState.animateScrollToPage(targetPage)
        }
    }

    LaunchedEffect(pagerState) {
        snapshotFlow { pagerState.isScrollInProgress to pagerState.currentPage }.collect { (scrolling, page) ->
            if (!scrolling) {
                val tab = tabForPage(page)
                if (tab != viewModel.selectedTab) {
                    viewModel.selectTab(tab)
                }
            }
        }
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(
                Brush.verticalGradient(
                    colors = listOf(NightBase, NightLayer, NightBase),
                )
            )
    ) {
        AppBackdrop()

        Scaffold(
            containerColor = Color.Transparent,
            bottomBar = {
                AppBottomBar(
                    selectedTab = selectedTab,
                    tabsEnabled = !pagerState.isScrollInProgress,
                    onSelectTab = viewModel::selectTab,
                )
            },
        ) { padding ->
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(padding)
                    .padding(horizontal = 16.dp, vertical = 12.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Text(
                    text = "P2P Share",
                    style = MaterialTheme.typography.headlineMedium,
                    color = NightText,
                )

                HorizontalPager(
                    state = pagerState,
                    modifier = Modifier
                        .fillMaxWidth()
                        .weight(1f),
                    pageSpacing = 12.dp,
                    beyondViewportPageCount = 1,
                ) { page ->
                    val pageTab = tabForPage(page)
                    val pageUi = when (pageTab) {
                        TransferTab.SEND -> sendUi
                        TransferTab.RECEIVE -> receiveUi
                        TransferTab.HISTORY -> TransferUiState(statusLine = "")
                    }

                    Column(
                        modifier = Modifier
                            .fillMaxSize()
                            .verticalScroll(rememberScrollState()),
                        verticalArrangement = Arrangement.spacedBy(14.dp),
                    ) {
                        AnimatedVisibility(
                            visible = !pageUi.ticket.isNullOrBlank() || !pageUi.qrPayload.isNullOrBlank(),
                            enter = fadeIn(animationSpec = tween(260)),
                            exit = fadeOut(animationSpec = tween(180)),
                        ) {
                            ConnectInviteCard(
                                ticket = pageUi.ticket,
                                qrPayload = pageUi.qrPayload,
                            )
                        }

                        TransferPanel(ui = pageUi, selectedTab = pageTab)

                        when (pageTab) {
                            TransferTab.SEND -> SendScreen(
                                statusLine = pageUi.statusLine,
                                connection = pageUi.connectionPath,
                                latencyMs = pageUi.latencyMs,
                                sendToTicket = send.sendToTicketMode,
                                ticketInput = send.ticketInput,
                                selectedFile = viewModel.describeFileUri(send.fileUri),
                                canCancelSend = viewModel.canCancelSend(),
                                showProgress = pageUi.stage == TransferStage.TRANSFERRING || pageUi.progressTotal > 0,
                                progressDone = pageUi.progressDone,
                                progressTotal = pageUi.progressTotal,
                                onToggleMode = viewModel::setSendMode,
                                onTicketChange = viewModel::setTicketInput,
                                onPickFile = { filePicker.launch(arrayOf("*/*")) },
                                onScanQr = {
                                    scanDestination = "send"
                                    qrScanner.launch(
                                        ScanOptions()
                                            .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                                            .setPrompt("Scan receiver ticket")
                                            .setBeepEnabled(false)
                                            .setCaptureActivity(PortraitCaptureActivity::class.java)
                                            .setOrientationLocked(true)
                                    )
                                },
                                onStart = viewModel::startSend,
                                onCancelSend = viewModel::cancelTransfer,
                            )

                            TransferTab.RECEIVE -> ReceiveScreen(
                                statusLine = pageUi.statusLine,
                                connection = pageUi.connectionPath,
                                latencyMs = pageUi.latencyMs,
                                connectMode = receive.connectMode,
                                target = receive.targetInput,
                                outputPath = viewModel.describeUri(receive.outputTreeUri),
                                canCancelReceive = viewModel.canCancelReceive(),
                                showProgress = pageUi.stage == TransferStage.TRANSFERRING || pageUi.progressTotal > 0,
                                progressDone = pageUi.progressDone,
                                progressTotal = pageUi.progressTotal,
                                onToggleMode = viewModel::setReceiveMode,
                                onTargetChange = viewModel::setTargetInput,
                                onPickOutput = { outputPicker.launch(null) },
                                onScanQr = {
                                    scanDestination = "receive"
                                    qrScanner.launch(
                                        ScanOptions()
                                            .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                                            .setPrompt("Scan sender ticket")
                                            .setBeepEnabled(false)
                                            .setCaptureActivity(PortraitCaptureActivity::class.java)
                                            .setOrientationLocked(true)
                                    )
                                },
                                onStart = viewModel::startReceive,
                                onCancelReceive = viewModel::cancelTransfer,
                            )

                            TransferTab.HISTORY -> HistoryScreen(viewModel.history)
                        }

                        if (!pageUi.errorMessage.isNullOrBlank()) {
                            DetailCard(
                                title = "Transfer error",
                                body = pageUi.errorMessage ?: "",
                                accent = SignalRed,
                            )
                        }

                        Spacer(Modifier.height(24.dp))
                    }
                }
            }
        }
    }
}

@Composable
private fun AppBackdrop() {
    Box(
        modifier = Modifier
            .fillMaxSize()
            .padding(0.dp)
    ) {
        Box(
            modifier = Modifier
                .align(Alignment.TopStart)
                .offset(x = (-60).dp, y = (-96).dp)
                .size(220.dp)
                .background(
                    brush = Brush.radialGradient(listOf(GlowBlue.copy(alpha = 0.36f), Color.Transparent)),
                    shape = CircleShape,
                )
        )
        Box(
            modifier = Modifier
                .align(Alignment.TopEnd)
                .offset(x = 74.dp, y = (-60).dp)
                .size(250.dp)
                .background(
                    brush = Brush.radialGradient(listOf(GlowCyan.copy(alpha = 0.34f), Color.Transparent)),
                    shape = CircleShape,
                )
        )
        Box(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .offset(y = 120.dp)
                .size(300.dp)
                .background(
                    brush = Brush.radialGradient(listOf(SignalRed.copy(alpha = 0.15f), Color.Transparent)),
                    shape = CircleShape,
                )
        )
    }
}

@Composable
private fun AppBottomBar(
    selectedTab: TransferTab,
    tabsEnabled: Boolean,
    onSelectTab: (TransferTab) -> Unit,
) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = NightSurface.copy(alpha = 0.94f),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.42f)),
        shape = RoundedCornerShape(0.dp),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(horizontal = 10.dp, vertical = 10.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            AppTabItem(
                modifier = Modifier.weight(1f),
                selected = selectedTab == TransferTab.SEND,
                enabled = tabsEnabled,
                label = "SEND",
                icon = { Icon(Icons.AutoMirrored.Filled.Send, contentDescription = null) },
                onClick = { onSelectTab(TransferTab.SEND) },
            )
            AppTabItem(
                modifier = Modifier.weight(1f),
                selected = selectedTab == TransferTab.RECEIVE,
                enabled = tabsEnabled,
                label = "RECEIVE",
                icon = { Icon(Icons.Default.Download, contentDescription = null) },
                onClick = { onSelectTab(TransferTab.RECEIVE) },
            )
            AppTabItem(
                modifier = Modifier.weight(1f),
                selected = selectedTab == TransferTab.HISTORY,
                enabled = tabsEnabled,
                label = "HISTORY",
                icon = { Icon(Icons.Default.History, contentDescription = null) },
                onClick = { onSelectTab(TransferTab.HISTORY) },
            )
        }
    }
}

@Composable
private fun AppTabItem(
    modifier: Modifier = Modifier,
    selected: Boolean,
    enabled: Boolean,
    label: String,
    icon: @Composable () -> Unit,
    onClick: () -> Unit,
) {
    Surface(
        modifier = modifier
            .height(56.dp)
            .clip(RoundedCornerShape(14.dp))
            .clickable(enabled = enabled, onClick = onClick),
        shape = RoundedCornerShape(14.dp),
        color = if (selected) {
            MaterialTheme.colorScheme.primary.copy(alpha = 0.14f)
        } else {
            MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.2f)
        },
        border = BorderStroke(
            1.dp,
            if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.45f)
            else MaterialTheme.colorScheme.outline.copy(alpha = 0.18f),
        ),
    ) {
        Column(
            modifier = Modifier.fillMaxSize(),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            val tint = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant
            Box(
                modifier = Modifier
                    .height(3.dp)
                    .fillMaxWidth(0.3f)
                    .background(if (selected) tint.copy(alpha = 0.85f) else Color.Transparent, CircleShape)
            )
            Spacer(Modifier.height(4.dp))
            androidx.compose.runtime.CompositionLocalProvider(
                androidx.compose.material3.LocalContentColor provides tint,
            ) {
                icon()
            }
            Text(
                text = label,
                style = MaterialTheme.typography.labelMedium,
                color = tint,
            )
        }
    }
}

@Composable
private fun GlassCard(
    modifier: Modifier = Modifier,
    accent: Color,
    label: String,
    content: @Composable ColumnScope.() -> Unit,
) {
    Card(
        modifier = modifier.fillMaxWidth(),
        shape = RoundedCornerShape(22.dp),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.5f)),
        colors = CardDefaults.cardColors(
            containerColor = Color.Transparent,
            contentColor = MaterialTheme.colorScheme.onSurface,
        ),
        elevation = CardDefaults.cardElevation(defaultElevation = 0.dp),
    ) {
        Box(
            modifier = Modifier.background(
                brush = Brush.linearGradient(
                    colors = listOf(
                        NightSurface.copy(alpha = 0.97f),
                        NightSurface.copy(alpha = 0.9f),
                        accent.copy(alpha = 0.2f),
                    ),
                    start = Offset(0f, 0f),
                    end = Offset(900f, 700f),
                )
            )
        ) {
            Column(
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                verticalArrangement = Arrangement.spacedBy(11.dp),
            ) {
                CardLabel(text = label)
                content()
            }
        }
    }
}

@Composable
private fun CardLabel(text: String) {
    Surface(
        shape = RoundedCornerShape(999.dp),
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.45f),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.45f)),
    ) {
        Text(
            text = text.uppercase(Locale.US),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 3.dp),
        )
    }
}

@Composable
private fun TransferPanel(ui: TransferUiState, selectedTab: TransferTab) {
    val showCompletedCard = when {
        selectedTab == TransferTab.HISTORY -> false
        ui.completedName == null -> false
        selectedTab == TransferTab.SEND -> ui.completedPath.isNullOrBlank()
        selectedTab == TransferTab.RECEIVE -> !ui.completedPath.isNullOrBlank()
        else -> false
    }

    AnimatedVisibility(visible = showCompletedCard, enter = fadeIn(), exit = fadeOut()) {
        val name = ui.completedName ?: return@AnimatedVisibility
        val size = ui.completedSize ?: 0
        val path = ui.completedPath
        if (selectedTab == TransferTab.SEND) {
            SendCompletedCard(name = name, size = size)
        } else if (selectedTab == TransferTab.RECEIVE && !path.isNullOrBlank()) {
            ReceiveCompletedCard(name = name, size = size, path = path)
        }
    }
}

@Composable
private fun TransferInfoZone(status: String, connection: String?, latencyMs: Double?) {
    val cleanedStatus = status.trim()
    val showStatus = cleanedStatus.isNotEmpty() && !cleanedStatus.equals("ready", ignoreCase = true)

    if (showStatus) {
        Surface(
            shape = RoundedCornerShape(14.dp),
            color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.34f),
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.3f)),
        ) {
            Text(
                text = cleanedStatus,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }

    if (!connection.isNullOrBlank()) {
        val connectionLabel = if (latencyMs == null) {
            connection
        } else {
            "$connection • ${"%.1f".format(latencyMs)} ms"
        }
        Surface(
            shape = RoundedCornerShape(14.dp),
            color = NeonCyan.copy(alpha = 0.09f),
            border = BorderStroke(1.dp, NeonCyan.copy(alpha = 0.25f)),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text(
                text = connectionLabel,
                style = MaterialTheme.typography.bodySmall,
                color = NeonCyan,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 9.dp),
            )
        }
    }
}

@Composable
private fun SendScreen(
    statusLine: String,
    connection: String?,
    latencyMs: Double?,
    sendToTicket: Boolean,
    ticketInput: String,
    selectedFile: String,
    canCancelSend: Boolean,
    showProgress: Boolean,
    progressDone: Long,
    progressTotal: Long,
    onToggleMode: (Boolean) -> Unit,
    onTicketChange: (String) -> Unit,
    onPickFile: () -> Unit,
    onScanQr: () -> Unit,
    onStart: () -> Unit,
    onCancelSend: () -> Unit,
) {
    GlassCard(accent = ElectricBlue, label = "Send") {
        Text("Share a file", style = MaterialTheme.typography.titleLarge)
        Text(
            "Choose a file, then share by receiver ticket or by waiting for a connection.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        TransferInfoZone(status = statusLine, connection = connection, latencyMs = latencyMs)

        ModeToggle(
            leftLabel = "Wait receiver",
            rightLabel = "Use ticket",
            rightSelected = sendToTicket,
            onToggle = onToggleMode,
        )

        SecondaryActionButton(
            label = "Choose file",
            icon = Icons.Default.AttachFile,
            onClick = onPickFile,
        )

        PathStrip(
            text = truncateFileNameKeepExtension(selectedFile),
            fallback = "No file selected",
            maxLines = 1,
        )

        if (sendToTicket) {
            OutlinedTextField(
                value = ticketInput,
                onValueChange = onTicketChange,
                label = { Text("Receiver ticket") },
                textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                colors = appTextFieldColors(),
            )
            SecondaryActionButton(
                label = "Scan receiver QR",
                icon = Icons.Default.QrCodeScanner,
                onClick = onScanQr,
            )
        }

        if (showProgress) {
            InlineProgress(done = progressDone, total = progressTotal)
        }

        PrimaryActionButton(
            label = "Start sending",
            onClick = onStart,
            enabled = !canCancelSend,
            icon = Icons.AutoMirrored.Filled.Send,
        )

        AnimatedVisibility(visible = canCancelSend, enter = fadeIn(), exit = fadeOut()) {
            DangerActionButton(
                label = "Cancel send",
                onClick = onCancelSend,
            )
        }
    }
}

@Composable
private fun ReceiveScreen(
    statusLine: String,
    connection: String?,
    latencyMs: Double?,
    connectMode: Boolean,
    target: String,
    outputPath: String,
    canCancelReceive: Boolean,
    showProgress: Boolean,
    progressDone: Long,
    progressTotal: Long,
    onToggleMode: (Boolean) -> Unit,
    onTargetChange: (String) -> Unit,
    onPickOutput: () -> Unit,
    onScanQr: () -> Unit,
    onStart: () -> Unit,
    onCancelReceive: () -> Unit,
) {
    GlassCard(accent = NeonCyan, label = "Receive") {
        Text("Accept a file", style = MaterialTheme.typography.titleLarge)
        Text(
            "Connect to a sender ticket or start listening and share your QR code.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        TransferInfoZone(status = statusLine, connection = connection, latencyMs = latencyMs)

        ModeToggle(
            leftLabel = "Connect target",
            rightLabel = "Listen + QR",
            rightSelected = !connectMode,
            onToggle = { onToggleMode(!it) },
        )

        if (connectMode) {
            OutlinedTextField(
                value = target,
                onValueChange = onTargetChange,
                label = { Text("Ticket or ip:port") },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                colors = appTextFieldColors(),
            )
        }

        SecondaryActionButton(
            label = "Scan sender QR",
            icon = Icons.Default.QrCodeScanner,
            onClick = onScanQr,
        )

        SecondaryActionButton(
            label = "Choose output folder",
            icon = Icons.Default.FolderOpen,
            onClick = onPickOutput,
        )

        PathStrip(
            text = truncateMiddle(outputPath, head = 34, tail = 20),
            fallback = "No output folder selected",
        )

        if (showProgress) {
            InlineProgress(done = progressDone, total = progressTotal)
        }

        PrimaryActionButton(
            label = "Start receiving",
            icon = Icons.Default.Download,
            onClick = onStart,
        )

        AnimatedVisibility(visible = canCancelReceive, enter = fadeIn(), exit = fadeOut()) {
            DangerActionButton(
                label = "Cancel transfer",
                onClick = onCancelReceive,
            )
        }
    }
}

@Composable
private fun HistoryScreen(entries: List<TransferHistoryEntry>) {
    GlassCard(accent = AccentViolet, label = "History") {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("Transfer log", style = MaterialTheme.typography.titleLarge)
            Text(
                "${entries.size} entries",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        if (entries.isEmpty()) {
            Text(
                "No transfers yet.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            return@GlassCard
        }

        LazyColumn(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.heightIn(max = 390.dp),
        ) {
            items(entries) { entry ->
                HistoryRow(entry)
            }
        }
    }
}

@Composable
private fun HistoryRow(entry: TransferHistoryEntry) {
    val isSend = entry.direction.equals("send", ignoreCase = true)
    val directionAccent = if (isSend) ElectricBlue else NeonCyan
    val directionIcon = if (isSend) Icons.Default.ArrowUpward else Icons.Default.ArrowDownward

    Surface(
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.28f),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.3f)),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 11.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Surface(
                shape = RoundedCornerShape(10.dp),
                color = directionAccent.copy(alpha = 0.14f),
                border = BorderStroke(1.dp, directionAccent.copy(alpha = 0.34f)),
            ) {
                Icon(
                    imageVector = directionIcon,
                    contentDescription = null,
                    tint = directionAccent,
                    modifier = Modifier.padding(8.dp),
                )
            }

            Column(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                Text(
                    text = entry.fileName,
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = pathPreview(entry.path),
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = "${entry.direction.uppercase(Locale.US)} • ${humanBytes(entry.sizeBytes)} • ${formatTime(entry.timestampMs)}",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (!entry.success && entry.detail.isNotBlank()) {
                    Text(
                        text = entry.detail,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            StatusPill(success = entry.success)
        }
    }
}

@Composable
private fun StatusPill(success: Boolean) {
    val accent = if (success) SignalGreen else SignalRed
    Surface(
        shape = RoundedCornerShape(999.dp),
        color = accent.copy(alpha = 0.16f),
        border = BorderStroke(1.dp, accent.copy(alpha = 0.34f)),
    ) {
        Text(
            text = if (success) "DONE" else "FAILED",
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 4.dp),
            style = MaterialTheme.typography.labelMedium,
            color = accent,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

@Composable
private fun InlineProgress(done: Long, total: Long) {
    val fraction = if (total <= 0L) 0f else (done.toFloat() / total.toFloat()).coerceIn(0f, 1f)
    Text("Transfer progress", style = MaterialTheme.typography.titleMedium)
    Surface(
        shape = RoundedCornerShape(999.dp),
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.6f),
        modifier = Modifier
            .fillMaxWidth()
            .height(10.dp),
    ) {
        Box(modifier = Modifier.fillMaxSize()) {
            Box(
                modifier = Modifier
                    .fillMaxWidth(fraction)
                    .height(10.dp)
                    .background(NeonCyan)
            )
        }
    }
    Text(
        "${humanBytes(done)} / ${humanBytes(total)}",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun DetailCard(title: String, body: String, accent: Color) {
    GlassCard(accent = accent, label = "Error") {
        Row(
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Default.Warning, contentDescription = null, tint = accent)
            Text(title, style = MaterialTheme.typography.titleMedium, color = accent)
        }
        Text(body, style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun ConnectInviteCard(ticket: String?, qrPayload: String?) {
    val clipboard = LocalClipboardManager.current
    val payload = qrPayload?.takeIf { it.isNotBlank() } ?: ticket?.takeIf { it.isNotBlank() }
    val displayTicket = ticket?.takeIf { it.isNotBlank() } ?: payload
    val bitmap = remember(payload) { payload?.let(::generateQrBitmap) }

    GlassCard(accent = NeonCyan, label = "Connect") {
        Text("Scan to connect", style = MaterialTheme.typography.titleMedium)

        if (bitmap != null) {
            Surface(
                shape = RoundedCornerShape(18.dp),
                color = Color.White,
                modifier = Modifier.align(Alignment.CenterHorizontally),
            ) {
                Image(
                    bitmap = bitmap.asImageBitmap(),
                    contentDescription = "Transfer QR",
                    modifier = Modifier
                        .padding(10.dp)
                        .size(214.dp),
                )
            }
        } else {
            Text("QR unavailable", color = MaterialTheme.colorScheme.error)
        }

        if (!displayTicket.isNullOrBlank()) {
            Text("Connection ticket", style = MaterialTheme.typography.titleSmall)
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = truncateMiddle(displayTicket),
                    style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                    modifier = Modifier.weight(1f),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                IconButton(onClick = { clipboard.setText(AnnotatedString(displayTicket)) }) {
                    Icon(Icons.Default.ContentCopy, contentDescription = "Copy ticket")
                }
            }
        }
    }
}

@Composable
private fun SendCompletedCard(name: String, size: Long) {
    GlassCard(accent = SignalGreen, label = "Completed") {
        Text("File sent", style = MaterialTheme.typography.titleMedium, color = SignalGreen)
        Text(
            "${truncateMiddle(name, head = 34, tail = 0)} (${humanBytes(size)})",
            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun ReceiveCompletedCard(name: String, size: Long, path: String) {
    GlassCard(accent = SignalGreen, label = "Completed") {
        Text("File received", style = MaterialTheme.typography.titleMedium, color = SignalGreen)
        Text(
            "${truncateMiddle(name, head = 32, tail = 0)} (${humanBytes(size)})",
            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            completedPathPreview(path, name),
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun PathStrip(text: String, fallback: String, maxLines: Int = 2) {
    Surface(
        shape = RoundedCornerShape(13.dp),
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.32f),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.3f)),
    ) {
        Text(
            text = if (text.isBlank()) fallback else text,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 9.dp),
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = maxLines,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun ModeToggle(
    leftLabel: String,
    rightLabel: String,
    rightSelected: Boolean,
    onToggle: (Boolean) -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.43f), RoundedCornerShape(14.dp))
            .padding(4.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        ToggleChip(
            label = leftLabel,
            selected = !rightSelected,
            modifier = Modifier.weight(1f),
            onClick = { onToggle(false) },
        )
        ToggleChip(
            label = rightLabel,
            selected = rightSelected,
            modifier = Modifier.weight(1f),
            onClick = { onToggle(true) },
        )
    }
}

@Composable
private fun ToggleChip(label: String, selected: Boolean, modifier: Modifier, onClick: () -> Unit) {
    Surface(
        modifier = modifier.clickable(onClick = onClick),
        color = if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.17f) else Color.Transparent,
        border = BorderStroke(
            1.dp,
            if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.42f) else Color.Transparent,
        ),
        shape = RoundedCornerShape(10.dp),
    ) {
        Box(
            modifier = Modifier
                .padding(vertical = 9.dp, horizontal = 10.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = label,
                style = MaterialTheme.typography.labelLarge,
                color = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun PrimaryActionButton(
    label: String,
    onClick: () -> Unit,
    enabled: Boolean = true,
    icon: ImageVector = Icons.Default.PlayArrow,
) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.fillMaxWidth(),
        colors = ButtonDefaults.buttonColors(
            containerColor = BottomGlowMauve,
            contentColor = NightText,
            disabledContainerColor = BottomGlowMauve.copy(alpha = 0.35f),
            disabledContentColor = NightText.copy(alpha = 0.7f),
        ),
        shape = RoundedCornerShape(14.dp),
    ) {
        Icon(icon, contentDescription = null)
        Spacer(Modifier.size(8.dp))
        Text(label, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun SecondaryActionButton(label: String, icon: ImageVector, onClick: () -> Unit) {
    OutlinedButton(
        onClick = onClick,
        modifier = Modifier.fillMaxWidth(),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline.copy(alpha = 0.64f)),
        colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.onSurface),
        shape = RoundedCornerShape(14.dp),
    ) {
        Icon(icon, contentDescription = null)
        Spacer(Modifier.size(8.dp))
        Text(label)
    }
}

@Composable
private fun DangerActionButton(label: String, onClick: () -> Unit, icon: ImageVector = Icons.Default.Cancel) {
    OutlinedButton(
        onClick = onClick,
        modifier = Modifier.fillMaxWidth(),
        border = BorderStroke(1.dp, SignalRed.copy(alpha = 0.64f)),
        colors = ButtonDefaults.outlinedButtonColors(contentColor = SignalRed),
        shape = RoundedCornerShape(14.dp),
    ) {
        Icon(icon, contentDescription = null)
        Spacer(Modifier.size(8.dp))
        Text(label)
    }
}

@Composable
private fun appTextFieldColors() = OutlinedTextFieldDefaults.colors(
    focusedTextColor = MaterialTheme.colorScheme.onSurface,
    unfocusedTextColor = MaterialTheme.colorScheme.onSurface,
    focusedBorderColor = NeonCyan,
    unfocusedBorderColor = MaterialTheme.colorScheme.outline,
    cursorColor = NeonCyan,
    focusedContainerColor = MaterialTheme.colorScheme.surface.copy(alpha = 0.52f),
    unfocusedContainerColor = MaterialTheme.colorScheme.surface.copy(alpha = 0.35f),
    focusedLabelColor = NeonCyan,
    unfocusedLabelColor = MaterialTheme.colorScheme.onSurfaceVariant,
)

private fun pageForTab(tab: TransferTab): Int = when (tab) {
    TransferTab.SEND -> 0
    TransferTab.RECEIVE -> 1
    TransferTab.HISTORY -> 2
}

private fun tabForPage(page: Int): TransferTab = when (page) {
    0 -> TransferTab.SEND
    1 -> TransferTab.RECEIVE
    else -> TransferTab.HISTORY
}

private fun generateQrBitmap(content: String): Bitmap? {
    return runCatching {
        val hints = mapOf(EncodeHintType.MARGIN to 1)
        val matrix = QRCodeWriter().encode(content, BarcodeFormat.QR_CODE, 900, 900, hints)
        val width = matrix.width
        val height = matrix.height
        val pixels = IntArray(width * height)

        for (y in 0 until height) {
            for (x in 0 until width) {
                pixels[y * width + x] = if (matrix[x, y]) 0xFF0A0E16.toInt() else 0xFFFFFFFF.toInt()
            }
        }

        Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888).apply {
            setPixels(pixels, 0, width, 0, 0, width, height)
        }
    }.getOrNull()
}

private fun formatTime(timestamp: Long): String {
    val fmt = SimpleDateFormat("MMM d, HH:mm", Locale.getDefault())
    return fmt.format(Date(timestamp))
}

private fun truncateMiddle(text: String, head: Int = 26, tail: Int = 18): String {
    if (tail <= 0) {
        if (text.length <= head) return text
        return "${text.take(head)}..."
    }
    if (text.length <= head + tail + 3) return text
    return "${text.take(head)}...${text.takeLast(tail)}"
}

private fun truncateFileNameKeepExtension(fileName: String, maxChars: Int = 34): String {
    val clean = fileName.trim()
    if (clean.isBlank() || clean.length <= maxChars) return clean

    val dot = clean.lastIndexOf('.')
    val hasExtension = dot > 0 && dot < clean.lastIndex
    if (!hasExtension) {
        return truncateMiddle(clean, head = (maxChars - 3).coerceAtLeast(8), tail = 0)
    }

    val extension = clean.substring(dot)
    val minStart = 6
    val maxExtensionLen = maxChars - 3 - minStart
    if (maxExtensionLen <= 1) {
        return truncateMiddle(clean, head = (maxChars - 3).coerceAtLeast(8), tail = 0)
    }

    val shownExtension = if (extension.length <= maxExtensionLen) {
        extension
    } else {
        ".${extension.removePrefix(".").takeLast(maxExtensionLen - 1)}"
    }

    val startLen = (maxChars - 3 - shownExtension.length).coerceAtLeast(minStart)
    if (startLen + 3 + shownExtension.length >= clean.length) return clean
    return "${clean.take(startLen)}...$shownExtension"
}

private fun pathPreview(path: String?): String {
    if (path.isNullOrBlank()) return "Path unavailable"
    val normalized = path.replace('\\', '/').trim()
    return truncateMiddle(normalized, head = 32, tail = 24)
}

private fun completedPathPreview(path: String, preferredName: String): String {
    val normalized = path.replace('\\', '/').trim()
    val split = normalized.lastIndexOf('/')
    val fileName = if (split >= 0) normalized.substring(split + 1) else normalized
    val finalName = if (fileName.isBlank()) preferredName else fileName
    val parent = if (split > 0) normalized.substring(0, split) else ""
    val shownName = truncateMiddle(finalName, head = 28, tail = 0)

    if (parent.isBlank()) return shownName
    if (parent.length <= 24) return "$shownName @ $parent"
    return "$shownName @ ...${parent.takeLast(21)}"
}

private fun humanBytes(bytes: Long): String {
    val kib = 1024.0
    val mib = kib * 1024.0
    val gib = mib * 1024.0
    val b = bytes.toDouble()
    return when {
        b < kib -> "$bytes B"
        b < mib -> String.format(Locale.US, "%.2f KiB", b / kib)
        b < gib -> String.format(Locale.US, "%.2f MiB", b / mib)
        else -> String.format(Locale.US, "%.2f GiB", b / gib)
    }
}
