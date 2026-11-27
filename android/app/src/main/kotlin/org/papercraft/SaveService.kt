package org.papercraft

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import androidx.activity.result.contract.ActivityResultContract
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException
import java.security.MessageDigest

/** Why a save didn't happen. The working copy ([Drafts]) is kept, so the edit isn't lost. */
sealed class SaveException(message: String, cause: Throwable? = null) : Exception(message, cause) {
    /** Papercraft doesn't write this format. */
    class Unsupported(val ext: String) : SaveException("Papercraft can't save .$ext files.")

    /** The original's format can't be written back (an old binary format); "Save as" [target] instead. */
    class NeedsSaveAs(val target: SaveTarget) : SaveException("This file can only be saved as a new .${target.ext} file.")

    /** We have no write access to the file; "Save as" instead. */
    class ReadOnly(cause: Throwable? = null) : SaveException("This file is read-only here. Save a copy instead.", cause)

    /** The file was changed by something else after we opened it; saving would overwrite that. */
    class ChangedOnDisk(val opened: FileStamp, val now: FileStamp) :
        SaveException("The file was changed somewhere else since you opened it.")

    /** The file was moved or deleted. */
    class Missing(cause: Throwable? = null) : SaveException("The file was moved or deleted.", cause)

    /** The provider ended up with a different size than we wrote (a failed upload, or it ignored truncation). */
    class Incomplete(val expected: Long, val actual: Long) :
        SaveException("The file didn't save completely ($actual of $expected bytes).")

    /** Writing failed. */
    class WriteFailed(cause: Throwable) : SaveException("The file couldn't be saved.", cause)
}

/** A working copy left over from a session that ended without saving: offer to restore it. */
class Draft(val file: DocFile, val savedAt: Long, val size: Long)

/** Crash-safe working copies of edited files, kept in app storage (no backup) until saved. Blocking: call off the main thread. */
class Drafts(context: Context) {
    private val store = DraftStore(File(context.noBackupFilesDir, "drafts"))

    private fun key(uri: Uri) = draftKey(uri.toString())

    /** Keeps [bytes] as the current edit of [file]. Call after edits; replaces the previous copy. */
    fun put(file: DocFile, bytes: ByteArray) = store.put(key(file.uri), file.encode(), bytes)

    /** The kept edit of [file], or null. */
    fun read(file: DocFile): ByteArray? = store.read(key(file.uri))

    /** Drops the kept edit of [uri], after a save or when the user discards changes. */
    fun discard(uri: Uri) = store.delete(key(uri))

    /** Unsaved edits from earlier sessions, newest first. [Draft.file] keeps the version stamp from when it was opened, so saving a recovered edit still catches outside changes. */
    fun pending(): List<Draft> = store.list().mapNotNull { e -> decodeDocFile(e.header)?.let { Draft(it, e.savedAt, e.size) } }
}

/** What "Save as" asks the system file picker for. [near] opens the picker in the original's folder, when the provider supports it. */
data class SaveAsRequest(val name: String, val mime: String, val near: Uri?)

/** The "Save as" request for [this] file: OOXML name and MIME for old binary formats. Null when Papercraft doesn't write the format. */
fun DocFile.saveAsRequest(): SaveAsRequest? =
    saveTargetOf(ext)?.let { SaveAsRequest(suggestedName(name, ext, it), it.mime, uri) }

/**
 * ActivityResultContracts.CreateDocument with the MIME type per launch instead of per contract, so one launcher serves every format.
 * Returns the new file's URI (with persistable read and write access), or null when cancelled.
 */
class CreateDocumentAs : ActivityResultContract<SaveAsRequest, Uri?>() {
    override fun createIntent(context: Context, input: SaveAsRequest): Intent =
        Intent(Intent.ACTION_CREATE_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType(input.mime)
            .putExtra(Intent.EXTRA_TITLE, input.name)
            .apply { input.near?.let { putExtra(DocumentsContract.EXTRA_INITIAL_URI, it) } }

    override fun parseResult(resultCode: Int, intent: Intent?): Uri? = intent.takeIf { resultCode == Activity.RESULT_OK }?.data
}

/**
 * Writes edited bytes back through the Storage Access Framework. Every call is blocking: run it off the main thread.
 * SAF can't rename over a file, so a save can't be atomic on the provider's side. What we do instead: the edit is in [drafts]
 * before the original is touched (when app storage has room), the original is written in one "wt" pass, and its size is
 * checked afterwards; the draft is dropped only when that check passes.
 */
class SaveService(private val context: Context) {
    val drafts = Drafts(context)

    /**
     * Saves [bytes] over [file]. Returns [file] with its new version stamp: use it for the next save.
     * Limit: cloud providers that update last-modified after the upload finishes make the next save see an outside change; lift it by keeping the provider's post-upload stamp (re-query after DocumentsContract notifies the URI).
     */
    @Throws(SaveException::class)
    fun save(file: DocFile, bytes: ByteArray, overwrite: Boolean = false): DocFile {
        val target = saveTargetOf(file.ext) ?: throw SaveException.Unsupported(file.ext)
        if (!target.inPlace) throw SaveException.NeedsSaveAs(target)
        if (!file.writable) throw SaveException.ReadOnly()
        // A full app storage must not stop the user from saving to the real file.
        runCatching { drafts.put(file, bytes) }
        val now = current(file.uri)
        val ours = if (!overwrite && changedOnDisk(file.stamp, now)) stillOurs(file.uri) else null
        android.util.Log.i("PcSave", "save ${file.uri} opened=${file.stamp} now=$now ours=$ours known=${written.containsKey(file.uri)} keys=${written.keys}")
        if (ours == false) throw SaveException.ChangedOnDisk(file.stamp, now)
        val after = write(file.uri, bytes)
        written[file.uri] = sha256(bytes)
        drafts.discard(file.uri)
        android.util.Log.i("PcSave", "wrote ${bytes.size} after=$after")
        return file.copy(size = after.size, modified = after.modified)
    }

    /**
     * Writes [bytes] to [target], a new file from [CreateDocumentAs], and returns it as the file to keep editing.
     * [from] is the file the edit came from; its draft is kept until this succeeds.
     */
    @Throws(SaveException::class)
    fun saveAs(from: DocFile?, target: Uri, bytes: ByteArray): DocFile {
        if (from != null) runCatching { drafts.put(from, bytes) }
        context.keepAccess(target)
        write(target, bytes)
        written[target] = sha256(bytes)
        if (from != null) drafts.discard(from.uri)
        return context.describe(target)
    }

    /**
     * True when the file still holds exactly what this app last saved to it. On the emulator's Downloads
     * provider the last-modified time read right after our own save later changed with nobody else
     * touching the file, which looked like an outside change on the next save.
     */
    private fun stillOurs(uri: Uri): Boolean {
        val mine = written[uri] ?: return false
        val read = runCatching { context.contentResolver.openInputStream(uri)?.use { it.readBytes() } }
        android.util.Log.i("PcSave", "readback ${read.exceptionOrNull()} size=${read.getOrNull()?.size}")
        return read.getOrNull()?.let { sha256(it) }?.contentEquals(mine) == true
    }

    private fun current(uri: Uri): FileStamp = try {
        context.stampOf(uri)
    } catch (e: FileNotFoundException) {
        throw SaveException.Missing(e)
    } catch (e: SecurityException) {
        throw SaveException.ReadOnly(e)
    }

    /** Writes and verifies; returns the stamp after writing. */
    private fun write(uri: Uri, bytes: ByteArray): FileStamp {
        try {
            open(uri).use { out ->
                out.write(bytes)
                // Some providers ignore "t" and leave the old tail; pipes (cloud providers) can't be truncated or synced, the size check below covers them.
                runCatching { out.channel.truncate(bytes.size.toLong()) }
                runCatching { out.fd.sync() }
            }
        } catch (e: FileNotFoundException) {
            throw SaveException.Missing(e)
        } catch (e: SecurityException) {
            throw SaveException.ReadOnly(e)
        } catch (e: IOException) {
            throw SaveException.WriteFailed(e)
        }
        val after = current(uri)
        // Limit: when the provider reports no size and hands out a pipe, the write can't be verified; lift it by reading the file back (a download on cloud providers).
        val size = after.size.takeIf { it >= 0 }
            ?: runCatching { context.contentResolver.openFileDescriptor(uri, "r")?.use { it.statSize } }.getOrNull() ?: -1
        if (size >= 0 && size != bytes.size.toLong()) throw SaveException.Incomplete(bytes.size.toLong(), size)
        return after.copy(size = if (size >= 0) size else after.size)
    }

    private fun open(uri: Uri): ParcelFileDescriptor.AutoCloseOutputStream {
        val resolver = context.contentResolver
        // "wt" truncates; a few providers only take "w", and the truncate + size check cover those.
        val pfd = try {
            resolver.openFileDescriptor(uri, "wt")
        } catch (e: IllegalArgumentException) {
            resolver.openFileDescriptor(uri, "w")
        } ?: throw FileNotFoundException(uri.toString())
        return ParcelFileDescriptor.AutoCloseOutputStream(pfd)
    }

    private companion object {
        /** SHA-256 of what this process last saved to each file, for [stillOurs]. */
        val written = java.util.concurrent.ConcurrentHashMap<Uri, ByteArray>()

        fun sha256(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)
    }
}
