package com.akily.p2pshare.update

import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext

@Composable
fun UpdateRow(viewModel: UpdateViewModel, transferBusy: Boolean) {
    val context = LocalContext.current
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        viewModel.permissionResult(context.packageManager.canRequestPackageInstalls())
    }
    val installer = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { }
    val stage = viewModel.stage
    if (stage == UpdateStage.IDLE || stage == UpdateStage.CHECKING) return

    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text(
            text = if (stage == UpdateStage.READY && transferBusy) "Update ready. Finish the transfer to install."
                else viewModel.message ?: when (stage) {
                    UpdateStage.AVAILABLE -> "Version ${viewModel.update?.versionName} is available."
                    UpdateStage.DOWNLOADING -> "Downloading update..."
                    UpdateStage.READY -> "Update ready."
                    else -> "Could not check for updates."
                },
            modifier = Modifier.weight(1f),
            style = MaterialTheme.typography.bodyMedium,
        )
        TextButton(
            enabled = stage != UpdateStage.DOWNLOADING && !(stage == UpdateStage.READY && transferBusy),
            onClick = {
                when (stage) {
                    UpdateStage.AVAILABLE -> viewModel.download()
                    UpdateStage.ERROR -> viewModel.check()
                    UpdateStage.READY -> if (!transferBusy) {
                        try {
                            if (!context.packageManager.canRequestPackageInstalls()) {
                                permission.launch(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                                    Uri.parse("package:${context.packageName}")))
                            } else {
                                installer.launch(viewModel.installerIntent())
                            }
                        } catch (_: Exception) {
                            viewModel.installFailed()
                        }
                    }
                    else -> Unit
                }
            },
        ) {
            Text(when (stage) {
                UpdateStage.READY -> "Install"
                UpdateStage.ERROR -> "Retry"
                else -> "Update"
            })
        }
    }
}
