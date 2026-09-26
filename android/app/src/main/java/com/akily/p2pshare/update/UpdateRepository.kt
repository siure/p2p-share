package com.akily.p2pshare.update

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

class UpdateRepository(private val context: Context) {
    private fun connect(url: String): HttpURLConnection {
        val connection = URL(url).openConnection() as HttpURLConnection
        connection.connectTimeout = 15_000
        connection.readTimeout = 30_000
        connection.setRequestProperty("User-Agent", "P2P-Share-Android")
        connection.useCaches = false
        return connection
    }

    suspend fun check(): AndroidUpdate? = withContext(Dispatchers.IO) {
        val connection = connect("$RELEASES_URL/latest/download/android-update.json")
        try {
            // Older releases, or desktop-only releases, may not have Android metadata.
            if (connection.responseCode == 404) return@withContext null
            check(connection.responseCode == 200) { "Update check failed (${connection.responseCode})." }
            val bytes = connection.inputStream.use { input ->
                val buffer = ByteArray(64 * 1024 + 1)
                var count = 0
                while (count < buffer.size) {
                    val read = input.read(buffer, count, buffer.size - count)
                    if (read == -1) break
                    count += read
                }
                buffer.copyOf(count)
            }
            require(bytes.size <= 64 * 1024) { "Update metadata is too large." }
            val json = JSONObject(bytes.toString(Charsets.UTF_8))
            val update = AndroidUpdate(
                json.getLong("versionCode"), json.getString("versionName"),
                json.getString("url"), json.getLong("size"), json.getString("sha256"),
            )
            val installed = context.packageManager.getPackageInfo(context.packageName, 0)
            update.takeIf { it.versionCode > installed.longVersionCode }
        } finally {
            connection.disconnect()
        }
    }

    suspend fun download(update: AndroidUpdate): File = withContext(Dispatchers.IO) {
        val directory = File(context.cacheDir, "updates").apply { mkdirs() }
        val partial = File(directory, "update.part")
        val ready = File(directory, "update.apk")
        ready.delete()
        val connection = connect(update.url)
        try {
            check(connection.responseCode == 200) { "Update download failed (${connection.responseCode})." }
            val coroutine = currentCoroutineContext()
            connection.inputStream.use { input ->
                downloadVerified(input, partial, update) { coroutine.ensureActive() }
            }
            val apk = context.packageManager.getPackageArchiveInfo(partial.absolutePath, 0)
            require(apk?.packageName == context.packageName && apk.longVersionCode == update.versionCode) {
                "Downloaded APK does not match the release."
            }
            // Android's installer verifies the signing key against the installed app.
            check(partial.renameTo(ready)) { "Could not save the update." }
            ready
        } finally {
            connection.disconnect()
            partial.delete()
        }
    }
}
