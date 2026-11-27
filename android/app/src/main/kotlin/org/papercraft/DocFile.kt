package org.papercraft

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.compose.runtime.saveable.Saver
import java.io.FileNotFoundException

/**
 * A file we opened. [size] and [modified] (ms) are what the provider reported when we opened it, -1 when it doesn't say;
 * saving compares them with the file on disk to catch outside changes. [writable] is false when we only got read access
 * (the provider is read-only or the sending app granted read only): edits then need "Save as".
 */
data class DocFile(
    val uri: Uri,
    val name: String,
    val ext: String,
    val size: Long,
    val openedAt: Long,
    val modified: Long = -1,
    val writable: Boolean = false,
) {
    val stamp get() = FileStamp(size, modified)
}

internal fun DocFile.encode() =
    listOf(uri, name.replace('\t', ' ').replace('\n', ' '), ext, size, openedAt, modified, writable).joinToString("\t")

internal fun decodeDocFile(line: String): DocFile? {
    val f = line.split('\t').takeIf { it.size == 5 || it.size == 7 } ?: return null
    return DocFile(
        Uri.parse(f[0]), f[1], f[2], f[3].toLongOrNull() ?: -1, f[4].toLongOrNull() ?: 0,
        f.getOrNull(5)?.toLongOrNull() ?: -1, f.getOrNull(6) == "true",
    )
}

val DocFileSaver = Saver<DocFile?, String>(save = { it?.encode() }, restore = ::decodeDocFile)

/** Keeps access after restart: read and write when the provider allows it, else read only. */
fun Context.keepAccess(uri: Uri) {
    val rw = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
    runCatching { contentResolver.takePersistableUriPermission(uri, rw) }
        .recoverCatching { contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
}

/** What a provider query says about a file. */
private class Meta(val name: String?, val size: Long, val modified: Long, val flags: Int?)

/** One metadata query, never downloads. Null when the file is gone. Blocking. */
private fun Context.meta(uri: Uri): Meta? {
    // Only document providers know these columns; others may reject the whole query if asked for them.
    val doc = DocumentsContract.isDocumentUri(this, uri)
    val cols = if (doc) arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE, Document.COLUMN_LAST_MODIFIED, Document.COLUMN_FLAGS)
    else arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
    return contentResolver.query(uri, cols, null, null, null)?.use { c ->
        if (!c.moveToFirst()) return null
        fun col(name: String) = c.getColumnIndex(name).takeIf { it >= 0 && !c.isNull(it) }
        Meta(
            col(OpenableColumns.DISPLAY_NAME)?.let(c::getString),
            col(OpenableColumns.SIZE)?.let(c::getLong) ?: -1,
            col(Document.COLUMN_LAST_MODIFIED)?.let(c::getLong) ?: -1,
            col(Document.COLUMN_FLAGS)?.let(c::getInt),
        )
    }
}

/** The file's size and last-modified time now. Throws [FileNotFoundException] when it's gone. Blocking. */
fun Context.stampOf(uri: Uri): FileStamp {
    val m = meta(uri) ?: throw FileNotFoundException(uri.toString())
    return FileStamp(m.size, m.modified)
}

/** Name, type, size, version and write access. Blocking: call off the main thread. */
fun Context.describe(uri: Uri): DocFile {
    val m = runCatching { meta(uri) }.getOrNull()
    val n = m?.name ?: uri.lastPathSegment?.substringAfterLast('/') ?: "Untitled"
    val ext = n.substringAfterLast('.', "").lowercase().ifEmpty {
        MimeTypeMap.getSingleton().getExtensionFromMimeType(contentResolver.getType(uri)) ?: ""
    }
    val granted = checkCallingOrSelfUriPermission(uri, Intent.FLAG_GRANT_WRITE_URI_PERMISSION) == PackageManager.PERMISSION_GRANTED
    // Document providers say whether the file takes writes; for other providers the grant is all we know.
    val writable = granted && (m?.flags?.let { it and Document.FLAG_SUPPORTS_WRITE != 0 } ?: true)
    return DocFile(uri, n, ext, m?.size ?: -1, System.currentTimeMillis(), m?.modified ?: -1, writable)
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
