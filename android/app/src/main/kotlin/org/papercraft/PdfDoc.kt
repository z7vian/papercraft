package org.papercraft

import android.content.Context
import android.graphics.Bitmap
import android.net.Uri
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.papercraft.core.CoreException
import org.papercraft.core.PdfFile
import org.papercraft.core.PdfHit
import org.papercraft.core.PdfPageSize
import java.io.FileNotFoundException
import java.nio.ByteBuffer

/** The PDF is locked: ask for its password and call [PdfDoc.open] again. [wrongPassword] is true when the one given didn't fit. */
class PasswordProtectedException(val wrongPassword: Boolean) :
    Exception(if (wrongPassword) "That password doesn't open this PDF." else "This PDF is password-protected. Password support is coming later.")

/** A PDF drawn by PDFium in the Rust core, the same on every Android version. The core runs one PDFium call at a time. */
class PdfDoc private constructor(private val file: PdfFile) : Pages {
    override val pageCount = file.pageCount().toInt()

    override suspend fun render(index: Int, width: Int): Bitmap? = withContext(Dispatchers.IO) {
        val img = file.render(index.toUInt(), width.toUInt()) ?: return@withContext null
        Bitmap.createBitmap(img.width.toInt(), img.height.toInt(), Bitmap.Config.ARGB_8888)
            .apply { copyPixelsFromBuffer(ByteBuffer.wrap(img.pixels)) }
    }

    /** Page [index]'s size in points as drawn, or null past the last page. */
    fun pageSize(index: Int): PdfPageSize? = file.pageSize(index.toUInt())

    /** Page [index]'s text, or null past the last page. */
    suspend fun pageText(index: Int): String? = withContext(Dispatchers.IO) { file.pageText(index.toUInt()) }

    /** Every case-insensitive match of [query]; boxes are in points from the page's top-left corner, like [pageSize]. */
    suspend fun search(query: String): List<PdfHit> = withContext(Dispatchers.IO) { file.search(query) }

    override fun closeLater() = file.close()

    companion object {
        /** Blocking: reads the whole file. Throws [PasswordProtectedException] when [password] is missing or wrong. */
        fun open(context: Context, uri: Uri, password: String? = null): PdfDoc {
            val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: throw FileNotFoundException()
            return try {
                PdfDoc(PdfFile.open(bytes, password))
            } catch (e: CoreException.PasswordRequired) {
                throw PasswordProtectedException(wrongPassword = false)
            } catch (e: CoreException.WrongPassword) {
                throw PasswordProtectedException(wrongPassword = true)
            }
        }
    }
}
