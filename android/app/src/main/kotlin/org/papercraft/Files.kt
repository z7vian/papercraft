package org.papercraft

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.compose.runtime.saveable.Saver
import androidx.compose.ui.graphics.Color
import org.papercraft.core.CoreException
import java.io.FileNotFoundException

/** White text on each color is ≥4.5:1. */
enum class Category(val label: String, val color: Color) {
    Docs("Docs", Color(0xFF2B579A)),
    Sheets("Sheets", Color(0xFF217346)),
    Slides("Slides", Color(0xFFD35400)),
    Pdf("PDF", Color(0xFFD93025)),
    Images("Images", Color(0xFF7B1FA2)),
}

enum class Viewer { Text, Table, Pdf, Image, Web, WebArchive, None }

fun categoryOf(ext: String): Category? = when (ext) {
    "doc", "docx", "docm", "dot", "dotx", "dotm", "wps", "wpt", "txt", "rtf", "xml",
    "html", "htm", "mht", "mhtml" -> Category.Docs
    "xls", "xlsx", "xlsm", "xlt", "xltx", "xltm", "xla", "xlam", "et", "ett",
    "csv", "dbf", "dif", "prn" -> Category.Sheets
    "ppt", "pptx", "pptm", "pot", "potx", "potm", "pps", "ppsx", "ppsm", "dps", "dpt", "ts" -> Category.Slides
    "pdf" -> Category.Pdf
    "jpg", "jpeg", "png", "bmp", "tif", "tiff" -> Category.Images
    else -> null
}

fun viewerOf(ext: String): Viewer = when (ext) {
    "txt" -> Viewer.Text
    "csv" -> Viewer.Table
    "pdf" -> Viewer.Pdf
    "jpg", "jpeg", "png", "bmp", "tif", "tiff" -> Viewer.Image
    "html", "htm" -> Viewer.Web
    "mht", "mhtml" -> Viewer.WebArchive
    else -> Viewer.None
}

/** A file we opened. [size] is -1 when the provider doesn't say. */
data class DocFile(val uri: Uri, val name: String, val ext: String, val size: Long, val openedAt: Long)

private fun DocFile.encode() =
    listOf(uri, name.replace('\t', ' ').replace('\n', ' '), ext, size, openedAt).joinToString("\t")

private fun decodeDocFile(line: String): DocFile? {
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

fun Context.readBytes(uri: Uri): ByteArray =
    contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: throw FileNotFoundException()

class TooBigException : Exception("This file is too big to open for now.")

/** Turns an exception into a sentence a person can act on. */
fun friendlyError(t: Throwable): String = when (t) {
    is FileNotFoundException, is SecurityException ->
        "Papercraft can't reach this file anymore. It may have been moved or deleted."
    is OutOfMemoryError -> "This file is too big to open for now."
    is CoreException -> t.message ?: "This file looks damaged."
    else -> t.message ?: "Something went wrong opening this file."
}

/** Recently opened files, newest first. Tab-separated lines in SharedPreferences. */
class Recents(private val context: Context) {
    private val prefs = context.getSharedPreferences("recents", Context.MODE_PRIVATE)

    fun load(): List<DocFile> = prefs.getString("files", "")!!.lines().mapNotNull(::decodeDocFile)

    // Both read the saved list (not an in-memory copy) so two app windows don't overwrite each other.
    fun add(file: DocFile) = save(listOf(file.copy(openedAt = System.currentTimeMillis())) + load().filter { it.uri != file.uri })
    fun remove(file: DocFile) = save(load().filter { it.uri != file.uri })

    private fun save(new: List<DocFile>): List<DocFile> {
        val kept = new.take(50)
        // Give back file access for anything that fell off the list; Android caps how many grants an app keeps.
        val keptUris = kept.mapTo(HashSet()) { it.uri }
        load().filter { it.uri !in keptUris }.forEach {
            runCatching { context.contentResolver.releasePersistableUriPermission(it.uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        }
        prefs.edit().putString("files", kept.joinToString("\n") { it.encode() }).apply()
        return kept
    }
}
