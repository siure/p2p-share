package com.akily.p2pshare.update

import android.app.Application
import android.content.Intent
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.FileProvider
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.akily.p2pshare.BuildConfig
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import java.io.File

enum class UpdateStage { IDLE, CHECKING, AVAILABLE, DOWNLOADING, READY, ERROR }

class UpdateViewModel(app: Application) : AndroidViewModel(app) {
    private val repository = UpdateRepository(app)
    private var apk: File? = null
    var update by mutableStateOf<AndroidUpdate?>(null)
        private set
    var stage by mutableStateOf(UpdateStage.IDLE)
        private set
    var message by mutableStateOf<String?>(null)
        private set

    init { if (!BuildConfig.DEBUG) check() }

    fun check() {
        if (stage != UpdateStage.IDLE && stage != UpdateStage.ERROR) return
        stage = UpdateStage.CHECKING
        message = null
        viewModelScope.launch {
            try {
                update = repository.check()
                stage = if (update == null) UpdateStage.IDLE else UpdateStage.AVAILABLE
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                stage = UpdateStage.ERROR
                message = "Could not check for updates."
            }
        }
    }

    fun download() {
        val release = update ?: return
        if (stage != UpdateStage.AVAILABLE) return
        stage = UpdateStage.DOWNLOADING
        message = null
        viewModelScope.launch {
            try {
                apk = repository.download(release)
                stage = UpdateStage.READY
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                stage = UpdateStage.AVAILABLE
                message = "Download failed. Try again."
            }
        }
    }

    fun installerIntent(): Intent {
        val app = getApplication<Application>()
        val file = requireNotNull(apk)
        check(stage == UpdateStage.READY && file.isFile) { "Download the update again." }
        val uri = FileProvider.getUriForFile(app, "${app.packageName}.updates", file)
        return Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }

    fun installFailed() {
        if (apk?.isFile != true) stage = UpdateStage.AVAILABLE
        message = "Could not open the installer. Try again."
    }

    fun permissionResult(allowed: Boolean) {
        message = if (allowed) "Update ready. Tap Install." else "Allow app installs to update."
    }
}
