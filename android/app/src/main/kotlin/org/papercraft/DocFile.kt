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
private class Meta(val name: String?, val size: Long, val modified: Long)

/** One metadata query, never downloads. Null when the file is gone. Blocking. */
private fun Context.meta(uri: Uri): Meta? {
    // Only document providers know these columns; others may reject the whole query if asked for them.
    val doc = DocumentsContract.isDocumentUri(this, uri)
    val cols = if (doc) arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE, Document.COLUMN_LAST_MODIFIED)
    else arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
    return contentResolver.query(uri, cols, null, null, null)?.use { c ->
        if (!c.moveToFirst()) return null
        fun col(name: String) = c.getColumnIndex(name).takeIf { it >= 0 && !c.isNull(it) }
        Meta(
            col(OpenableColumns.DISPLAY_NAME)?.let(c::getString),
            col(OpenableColumns.SIZE)?.let(c::getLong) ?: -1,
            col(Document.COLUMN_LAST_MODIFIED)?.let(c::getLong) ?: -1,
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
    // The write grant decides. Document flags are only hints: on this emulator the Downloads picker granted
    // write while the app still saw the file as read-only, and a provider that truly refuses makes the save
    // fail with SaveException.ReadOnly.
    val writable = checkCallingOrSelfUriPermission(uri, Intent.FLAG_GRANT_WRITE_URI_PERMISSION) == PackageManager.PERMISSION_GRANTED
    return DocFile(uri, n, ext, m?.size ?: -1, System.currentTimeMillis(), m?.modified ?: -1, writable)
}

/** True if the file still exists and we may read it. Cheap metadata query, never downloads. Blocking. */
fun Context.isReachable(uri: Uri): Boolean = runCatching {
    contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { it.moveToFirst() } == true
}.getOrDefault(false)

class TooBigException : Exception()

/** Default cap for [readBytes]: a quarter of the heap, so the bytes and what is decoded from them still fit. */
val defaultMaxBytes: Long get() = Runtime.getRuntime().maxMemory() / 4

/** The whole file. Throws [TooBigException] past [maxBytes], before reading when the provider reports the size. Blocking. */
fun Context.readBytes(file: DocFile, maxBytes: Long = defaultMaxBytes): ByteArray = readBytes(file.uri, file.size, maxBytes)

/**
 * The whole content at [uri]. Throws [TooBigException] past [maxBytes]: before reading when [knownSize] (-1: unknown)
 * says so, else as soon as the stream passes it, so a provider that lies or doesn't say never fills the heap. Blocking.
 */
fun Context.readBytes(uri: Uri, knownSize: Long = -1, maxBytes: Long = defaultMaxBytes): ByteArray {
    val cap = minOf(maxBytes, Int.MAX_VALUE - 8L) // largest array the VM allocates
    if (knownSize > cap) throw TooBigException()
    val stream = contentResolver.openInputStream(uri) ?: throw FileNotFoundException()
    return stream.use {
        // Sized right when the provider reports the size, so a correct report reads with no copy.
        var bytes = ByteArray(if (knownSize >= 0) knownSize.toInt() else minOf(cap, CHUNK).toInt())
        var len = 0
        while (true) {
            if (len == bytes.size) {
                val next = it.read()
                if (next < 0) return@use bytes
                if (len >= cap) throw TooBigException()
                bytes = bytes.copyOf(minOf(maxOf(len * 2L, CHUNK), cap).toInt())
                bytes[len++] = next.toByte()
            }
            val n = it.read(bytes, len, bytes.size - len)
            if (n < 0) break
            len += n
        }
        if (len == bytes.size) bytes else bytes.copyOf(len)
    }
}

private const val CHUNK = 64L * 1024
