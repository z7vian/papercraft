package org.papercraft

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.compose.runtime.saveable.Saver
import java.io.FileNotFoundException

/** A file we opened. [size] is -1 when the provider doesn't say. */
data class DocFile(val uri: Uri, val name: String, val ext: String, val size: Long, val openedAt: Long)

internal fun DocFile.encode() =
    listOf(uri, name.replace('\t', ' ').replace('\n', ' '), ext, size, openedAt).joinToString("\t")

internal fun decodeDocFile(line: String): DocFile? {
    val f = line.split('\t').takeIf { it.size == 5 } ?: return null
    return DocFile(Uri.parse(f[0]), f[1], f[2], f[3].toLongOrNull() ?: -1, f[4].toLongOrNull() ?: 0)
}

val DocFileSaver = Saver<DocFile?, String>(save = { it?.encode() }, restore = ::decodeDocFile)

/** Name, type and size in one provider query. Blocking: call off the main thread. */
fun Context.describe(uri: Uri): DocFile {
    var name: String? = null
    var size = -1L
    runCatching {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use {
            if (it.moveToFirst()) {
                name = it.getString(0)
                if (!it.isNull(1)) size = it.getLong(1)
            }
        }
    }
    val n = name ?: uri.lastPathSegment?.substringAfterLast('/') ?: "Untitled"
    val ext = n.substringAfterLast('.', "").lowercase().ifEmpty {
        MimeTypeMap.getSingleton().getExtensionFromMimeType(contentResolver.getType(uri)) ?: ""
    }
    return DocFile(uri, n, ext, size, System.currentTimeMillis())
}

/** True if the file still exists and we may read it. Cheap metadata query, never downloads. Blocking. */
fun Context.isReachable(uri: Uri): Boolean = runCatching {
    contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { it.moveToFirst() } == true
}.getOrDefault(false)

class TooBigException : Exception()

/** The whole file. Throws [TooBigException] past [maxBytes], before reading when the provider reports the size. Blocking. */
fun Context.readBytes(file: DocFile, maxBytes: Long = Long.MAX_VALUE): ByteArray {
    if (file.size > maxBytes) throw TooBigException()
    val bytes = contentResolver.openInputStream(file.uri)?.use { it.readBytes() } ?: throw FileNotFoundException()
    if (bytes.size > maxBytes) throw TooBigException()
    return bytes
}
