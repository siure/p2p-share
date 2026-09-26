package com.akily.p2pshare.storage

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.documentfile.provider.DocumentFile
import java.io.File
import java.io.IOException
import java.util.UUID

object FilePrep {
    fun copyUrisToCacheFiles(context: Context, uris: List<Uri>): List<File> {
        val copied = mutableListOf<File>()
        try {
            for (uri in uris) {
                copied += copyUriToCacheFile(context, uri)
            }
            return copied
        } catch (err: Throwable) {
            copied.forEach { deletePreparedFile(context, it) }
            throw err
        }
    }

    fun copyUriToCacheFile(context: Context, uri: Uri): File {
        val resolver = context.contentResolver
        val sourceMeta = resolver.queryOpenableMeta(uri)
        val name = sourceMeta?.displayName?.takeIf { it.isNotBlank() }
            ?: DocumentFile.fromSingleUri(context, uri)?.name ?: "shared-file.bin"
        val directory = File(context.cacheDir, "p2pshare-send-${UUID.randomUUID()}")
        check(directory.mkdirs()) { "Unable to create preparation directory" }
        val target = File(directory, File(name).name.takeIf { it.isNotBlank() && it != "." && it != ".." } ?: "shared-file.bin")
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
            deletePreparedFile(context, target)
            throw err
        }
    }

    fun deletePreparedFile(context: Context, file: File) {
        val parent = file.parentFile ?: return
        if (parent.parentFile?.canonicalFile != context.cacheDir.canonicalFile ||
            !parent.name.startsWith("p2pshare-send-")) return
        file.delete()
        parent.delete()
    }

    fun deleteExportedStaging(context: Context, sourcePath: String) {
        val source = File(sourcePath).canonicalFile
        val incoming = File(context.filesDir, "incoming").canonicalFile
        if (source.toPath().startsWith(incoming.toPath()) && source != incoming) {
            source.deleteRecursively()
        }
    }

    fun copyReceivedEntryToTree(
        context: Context,
        sourcePath: String,
        outputTree: Uri,
        requestedName: String,
    ): Uri? {
        val root = DocumentFile.fromTreeUri(context, outputTree) ?: return null
        val source = File(sourcePath)
        val usedNames = root.listFiles().mapNotNull { it.name }.toMutableSet()
        return when {
            source.isDirectory -> copyDirectoryToTree(context, source, root, requestedName, usedNames)?.uri
            source.isFile -> copyFileToTree(context, source, root, requestedName, usedNames)?.uri
            else -> null
        }
    }

    private fun copyDirectoryToTree(
        context: Context,
        sourceDir: File,
        root: DocumentFile,
        requestedName: String,
        usedNames: MutableSet<String>,
    ): DocumentFile? {
        val outDir = createUniqueDirectory(root, requestedName, usedNames) ?: return null
        val childNames = mutableSetOf<String>()
        val children = sourceDir.listFiles().orEmpty().sortedBy { it.name.lowercase() }
        for (child in children) {
            val copied = if (child.isDirectory) {
                copyDirectoryToTree(context, child, outDir, child.name, childNames)
            } else if (child.isFile) {
                copyFileToTree(context, child, outDir, child.name, childNames)
            } else {
                continue
            }
            if (copied == null) {
                return null
            }
        }
        return outDir
    }

    private fun copyFileToTree(
        context: Context,
        source: File,
        root: DocumentFile,
        requestedName: String,
        usedNames: MutableSet<String>,
    ): DocumentFile? {
        val candidateName = uniqueChildName(usedNames, requestedName)
        val mime = contentResolverMime(context.contentResolver, requestedName)
        val outDoc = root.createFile(mime, candidateName) ?: return null

        context.contentResolver.openOutputStream(outDoc.uri)?.use { out ->
            source.inputStream().use { input ->
                input.copyTo(out)
            }
        } ?: return null

        return outDoc
    }

    private fun createUniqueDirectory(root: DocumentFile, requestedName: String, usedNames: MutableSet<String>): DocumentFile? {
        val candidateName = uniqueChildName(usedNames, requestedName)
        return root.createDirectory(candidateName)
    }

    private fun uniqueChildName(usedNames: MutableSet<String>, requestedName: String): String {
        var candidateName = requestedName
        var index = 1
        while (!usedNames.add(candidateName)) {
            candidateName = withSuffixIndex(requestedName, index)
            index += 1
        }
        return candidateName
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
                val name =
                    if (nameIdx >= 0 && !cursor.isNull(nameIdx)) cursor.getString(nameIdx) else null
                val size =
                    if (sizeIdx >= 0 && !cursor.isNull(sizeIdx)) cursor.getLong(sizeIdx) else null
                OpenableMeta(name, size)
            }
        }.getOrNull()
    }
}
