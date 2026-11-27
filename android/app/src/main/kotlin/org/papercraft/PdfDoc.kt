package org.papercraft

import android.content.Context
import android.graphics.Bitmap
import android.graphics.pdf.PdfRenderer
import android.net.Uri
import android.os.ParcelFileDescriptor
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.FileNotFoundException
import kotlin.math.sqrt

class PasswordProtectedException : Exception("This PDF is password-protected. Password support is coming later.")

/** Android's built-in PDF renderer. It's not thread-safe, so every call goes through one lock. */
class PdfDoc private constructor(fd: ParcelFileDescriptor) : Pages {
    private val renderer = PdfRenderer(fd) // takes ownership of fd
    private val lock = Mutex()
    private var closed = false
    override val pageCount = renderer.pageCount

    override suspend fun render(index: Int, width: Int): Bitmap? = lock.withLock {
        if (closed) return null
        withContext(Dispatchers.IO) {
            renderer.openPage(index).use { page ->
                // Limit: max 16 MP (64 MB) per page, under Android's 100 MB draw limit; tile very tall pages to make them sharper.
                val w = minOf(width.toDouble(), sqrt(16e6 * page.width / page.height)).toInt().coerceAtLeast(1)
                val h = (w.toLong() * page.height / page.width).toInt().coerceAtLeast(1)
                Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888).also {
                    it.eraseColor(android.graphics.Color.WHITE)
                    page.render(it, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                }
            }
        }
    }

    override fun closeLater() {
        CoroutineScope(Dispatchers.IO).launch { lock.withLock { closed = true; renderer.close() } }
    }

    companion object {
        /** Blocking. PdfRenderer throws SecurityException for a password-protected file. */
        fun open(context: Context, uri: Uri): PdfDoc {
            val fd = context.contentResolver.openFileDescriptor(uri, "r") ?: throw FileNotFoundException()
            return try {
                PdfDoc(fd)
            } catch (e: SecurityException) {
                fd.close()
                throw PasswordProtectedException()
            } catch (t: Throwable) {
                fd.close()
                throw t
            }
        }
    }
}
