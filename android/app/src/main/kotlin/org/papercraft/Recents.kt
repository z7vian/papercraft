package org.papercraft

import android.content.Context
import android.content.Intent

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
            val rw = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            runCatching { context.contentResolver.releasePersistableUriPermission(it.uri, rw) }
            Thumbnails.delete(context, it.uri)
        }
        prefs.edit().putString("files", kept.joinToString("\n") { it.encode() }).apply()
        return kept
    }
}
