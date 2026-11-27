package org.papercraft

import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.InputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

/**
 * Edited bytes kept in app storage until they're saved, so a crash or a killed process loses nothing.
 * One `<key>.draft` file per document: a one-line [header] (who the bytes belong to), then the bytes.
 * Each [put] writes a temp file, syncs it and renames it over the old one, so a crash leaves the old copy or the new one, never half.
 * Plain JVM (no Android types) so the unit tests run it on real files. Blocking: call off the main thread.
 */
class DraftStore(private val dir: File) {
    class Entry(val key: String, val header: String, val savedAt: Long, val size: Long)

    fun put(key: String, header: String, bytes: ByteArray) {
        require('\n' !in header) { "header must be one line" }
        val target = fileOf(key)
        dir.mkdirs()
        val tmp = File.createTempFile("draft-$key-", TMP, dir) // prefix needs 3+ characters
        try {
            FileOutputStream(tmp).use {
                it.write(header.toByteArray())
                it.write('\n'.code)
                it.write(bytes)
                it.fd.sync()
            }
            Files.move(tmp.toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally {
            tmp.delete()
        }
    }

    /** The bytes saved under [key], or null if there are none. */
    fun read(key: String): ByteArray? {
        val f = fileOf(key)
        if (!f.isFile) return null
        return FileInputStream(f).buffered().use { if (it.readHeader() == null) null else it.readBytes() }
    }

    fun delete(key: String) {
        fileOf(key).delete()
    }

    /** Every draft, newest first. Also clears temp files a crash left behind. */
    fun list(): List<Entry> {
        val files = dir.listFiles() ?: return emptyList()
        val stale = System.currentTimeMillis() - STALE_TMP_MS
        files.filter { it.name.endsWith(TMP) && it.lastModified() < stale }.forEach { it.delete() }
        return files.filter { it.name.endsWith(DRAFT) }.mapNotNull { f ->
            val header = runCatching { FileInputStream(f).buffered().use { it.readHeader() } }.getOrNull() ?: return@mapNotNull null
            Entry(f.name.removeSuffix(DRAFT), header, f.lastModified(), f.length() - header.toByteArray().size - 1)
        }.sortedByDescending { it.savedAt }
    }

    private fun fileOf(key: String): File {
        require(key.isNotEmpty() && key.all { it.isLetterOrDigit() }) { "bad draft key" }
        return File(dir, key + DRAFT)
    }

    /** The first line, or null if the file has no complete one. */
    private fun InputStream.readHeader(): String? {
        val line = ByteArrayOutputStream()
        while (true) {
            val b = read()
            if (b == -1 || line.size() > MAX_HEADER) return null
            if (b == '\n'.code) return line.toString(Charsets.UTF_8.name())
            line.write(b)
        }
    }

    private companion object {
        const val DRAFT = ".draft"
        const val TMP = ".tmp"
        const val MAX_HEADER = 64 * 1024
        const val STALE_TMP_MS = 60_000L
    }
}

/** A file-name-safe key for [id] (a URI string): its SHA-256 in hex. */
fun draftKey(id: String): String =
    MessageDigest.getInstance("SHA-256").digest(id.toByteArray()).joinToString("") { "%02x".format(it) }
