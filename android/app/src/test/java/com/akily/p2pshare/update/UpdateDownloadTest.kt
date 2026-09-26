package com.akily.p2pshare.update

import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.security.MessageDigest

class UpdateDownloadTest {
    private val bytes = "signed APK fixture".toByteArray()
    private val release = AndroidUpdate(5000, "0.5.0",
        "$RELEASES_URL/download/v0.5.0/p2p-share-android-v0.5.0.apk", bytes.size.toLong(),
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) })

    @Test fun acceptsCompleteDownload() {
        val target = File.createTempFile("update", ".apk")
        try {
            downloadVerified(bytes.inputStream(), target, release)
            assertArrayEquals(bytes, target.readBytes())
        } finally { target.delete() }
    }

    @Test fun removesTruncatedOversizedAndCorruptedDownloads() {
        val invalid = listOf(bytes.dropLast(1).toByteArray(), bytes + byteArrayOf(0), ByteArray(bytes.size))
        invalid.forEach { content ->
            val target = File.createTempFile("update", ".apk")
            assertThrows(IllegalArgumentException::class.java) {
                downloadVerified(content.inputStream(), target, release)
            }
            assertFalse(target.exists())
        }
    }

    @Test fun removesCanceledDownloads() {
        val target = File.createTempFile("update", ".apk")
        assertThrows(IllegalStateException::class.java) {
            downloadVerified(bytes.inputStream(), target, release) { error("canceled") }
        }
        assertFalse(target.exists())
    }

    @Test fun rejectsUnexpectedAssetsAndMalformedMetadata() {
        assertThrows(IllegalArgumentException::class.java) { release.copy(url = "https://example.com/app.apk") }
        assertThrows(IllegalArgumentException::class.java) { release.copy(versionName = "../latest") }
        assertThrows(IllegalArgumentException::class.java) { release.copy(size = 0) }
        assertThrows(IllegalArgumentException::class.java) { release.copy(sha256 = "invalid") }
    }
}
