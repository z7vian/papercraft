package org.papercraft

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import java.io.File
import java.security.MessageDigest

/** Page-one previews of recent files, kept in the cache folder so Android may clear them when space runs low. */
object Thumbnails {
    private const val WIDTH = 360

    private fun file(context: Context, uri: Uri): File {
        val hash = MessageDigest.getInstance("SHA-1").digest(uri.toString().toByteArray()).joinToString("") { "%02x".format(it) }
        return File(File(context.cacheDir, "thumbs"), "$hash.webp")
    }

    /** Blocking. */
    fun save(context: Context, uri: Uri, page: Bitmap) {
        val f = file(context, uri)
        f.parentFile?.mkdirs()
        val h = (page.height.toLong() * WIDTH / page.width).toInt().coerceIn(1, WIDTH * 2)
        val small = Bitmap.createScaledBitmap(page, WIDTH, h, true)
        @Suppress("DEPRECATION")
        val format = if (android.os.Build.VERSION.SDK_INT >= 30) Bitmap.CompressFormat.WEBP_LOSSY else Bitmap.CompressFormat.WEBP
        f.outputStream().use { small.compress(format, 82, it) }
        if (small !== page) small.recycle()
    }

    /** Blocking; null when there is none yet. */
    fun load(context: Context, uri: Uri): Bitmap? = file(context, uri).takeIf { it.exists() }?.let { BitmapFactory.decodeFile(it.path) }

    fun delete(context: Context, uri: Uri) {
        file(context, uri).delete()
    }
}
