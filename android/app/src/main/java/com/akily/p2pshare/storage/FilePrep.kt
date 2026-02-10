package com.akily.p2pshare.storage

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.documentfile.provider.DocumentFile
import java.io.File
import java.io.IOException

object FilePrep {
    fun copyUriToCacheFile(context: Context, uri: Uri): File {
        val resolver = context.contentResolver
        val fallbackName = DocumentFile.fromSingleUri(context, uri)?.name ?: "shared-file.bin"
        val sourceMeta = resolver.queryOpenableMeta(uri)
        val name = sourceMeta?.displayName?.takeIf { it.isNotBlank() } ?: fallbackName
        val target = File(context.cacheDir, "send-${System.currentTimeMillis()}-$name")
        try {
            val copied = resolver.openInputStream(uri).use { input ->
                requireNotNull(input) { "Unable to read selected file" }
                target.outputStream().use { out ->
                    input.copyTo(out)
                }
            }

            val expectedSize = sourceMeta?.size
            if (expectedSize != null && expectedSize >= 0L && copied != expectedSize) {
                throw IOException(
                    "File copy incomplete (expected $expectedSize bytes, copied $copied bytes)"
                )
            }
            if (expectedSize != null && expectedSize > 0L && copied == 0L) {
                throw IOException("Selected file appears unreadable (copied 0 bytes)")
            }

            return target
        } catch (err: Throwable) {
            target.delete()
            throw err
        }
    }

    fun copyReceivedFileToTree(
        context: Context,
        sourcePath: String,
        outputTree: Uri,
        requestedName: String,
    ): Uri? {
        val root = DocumentFile.fromTreeUri(context, outputTree) ?: return null
        var candidateName = requestedName
        var index = 1
        while (root.findFile(candidateName) != null) {
            candidateName = withSuffixIndex(requestedName, index)
            index += 1
        }

        val mime = contentResolverMime(context.contentResolver, requestedName)
        val outDoc = root.createFile(mime, candidateName) ?: return null

        val source = File(sourcePath)
        context.contentResolver.openOutputStream(outDoc.uri)?.use { out ->
            source.inputStream().use { input ->
                input.copyTo(out)
            }
        } ?: return null

        return outDoc.uri
    }

    private fun withSuffixIndex(name: String, index: Int): String {
        val dot = name.lastIndexOf('.')
        return if (dot <= 0) {
            "$name ($index)"
        } else {
            val base = name.substring(0, dot)
            val ext = name.substring(dot)
            "$base ($index)$ext"
        }
    }

    private fun contentResolverMime(resolver: ContentResolver, name: String): String {
        val ext = name.substringAfterLast('.', "").lowercase()
        val type = android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)
        return type ?: "application/octet-stream"
    }

    private data class OpenableMeta(
        val displayName: String?,
        val size: Long?,
    )

    private fun ContentResolver.queryOpenableMeta(uri: Uri): OpenableMeta? {
        val projection = arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        return runCatching {
            query(uri, projection, null, null, null)?.use { cursor ->
                if (!cursor.moveToFirst()) return@use null
                val nameIdx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                val sizeIdx = cursor.getColumnIndex(OpenableColumns.SIZE)
                val name = if (nameIdx >= 0 && !cursor.isNull(nameIdx)) cursor.getString(nameIdx) else null
                val size = if (sizeIdx >= 0 && !cursor.isNull(sizeIdx)) cursor.getLong(sizeIdx) else null
                OpenableMeta(name, size)
            }
        }.getOrNull()
    }
}
