package com.akily.p2pshare.update

import java.io.File
import java.io.InputStream
import java.security.MessageDigest

const val RELEASES_URL = "https://github.com/siure/p2p-share/releases"

data class AndroidUpdate(
    val versionCode: Long,
    val versionName: String,
    val url: String,
    val size: Long,
    val sha256: String,
) {
    init {
        require(versionCode > 0 && versionCode <= 2_100_000_000)
        require(Regex("[0-9]+\\.[0-9]+\\.[0-9]+").matches(versionName))
        require(url == "$RELEASES_URL/download/v$versionName/p2p-share-android-v$versionName.apk")
        require(size in 1..500_000_000)
        require(Regex("[0-9a-f]{64}").matches(sha256))
    }
}

/** Never leave a partial or unverified APK available to the installer. */
fun downloadVerified(input: InputStream, target: File, update: AndroidUpdate, checkCanceled: () -> Unit = {}) {
    var verified = false
    try {
        val digest = MessageDigest.getInstance("SHA-256")
        var total = 0L
        target.outputStream().use { output ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
                checkCanceled()
                val count = input.read(buffer)
                if (count == -1) break
                total += count
                require(total <= update.size) { "Update exceeds its expected size." }
                digest.update(buffer, 0, count)
                output.write(buffer, 0, count)
            }
        }
        require(total == update.size) { "Update download is incomplete." }
        val hash = digest.digest().joinToString("") { "%02x".format(it) }
        require(hash == update.sha256) { "Update checksum does not match." }
        verified = true
    } finally {
        if (!verified) target.delete()
    }
}
