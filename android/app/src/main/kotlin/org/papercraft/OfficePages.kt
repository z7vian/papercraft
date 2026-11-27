package org.papercraft

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.papercraft.core.OfficeDoc
import org.papercraft.core.OfficeFamily
import java.util.concurrent.ConcurrentHashMap

/** Word, Excel and PowerPoint files (old binary ones too), drawn by the Rust core. */
class OfficePages private constructor(private val doc: OfficeDoc) : Pages {
    override val pageCount = doc.pageCount()?.toInt()
    private val names = doc.pageNames()

    // The core draws at a scale, not a width; pages in one file are mostly one width, so the last page's size sets the next scale.
    @Volatile private var naturalWidth = 800f

    // Page texts read once for find: a cancelled search may still be reading one while the next search starts.
    private val texts = ConcurrentHashMap<Int, String>()

    override fun title(index: Int): String? = names.getOrNull(index)

    override suspend fun render(index: Int, width: Int): Bitmap? = withContext(Dispatchers.IO) {
        val scale = (width / naturalWidth).coerceIn(0.25f, 4f)
        val png = doc.renderPage(index.toUInt(), scale) ?: return@withContext null
        val bitmap = BitmapFactory.decodeByteArray(png, 0, png.size) ?: error("This page looks damaged.")
        naturalWidth = bitmap.width / scale
        bitmap
    }

    /** The pages holding a case-insensitive match of [query], one hit per page; stops between pages once cancelled. */
    // Limit: hits name pages only, with no boxes; the core's page drawing would have to report where each run of text lands to lift it.
    override suspend fun find(query: String, limit: Int): List<PageHit> = withContext(Dispatchers.IO) {
        val hits = ArrayList<PageHit>()
        var index = 0
        // Documents learn their page count while drawing, so read pages until the core says there are no more.
        while (pageCount == null || index < pageCount) {
            ensureActive()
            val text = texts[index] ?: (doc.pageText(index.toUInt()) ?: break).also { texts[index] = it }
            if (text.contains(query, ignoreCase = true)) {
                hits += PageHit(index, emptyList())
                if (hits.size >= limit) break
            }
            index++
        }
        hits
    }

    override fun closeLater() = doc.close()

    companion object {
        /** Blocking: reads and parses the whole file. */
        fun open(context: Context, file: DocFile): OfficePages {
            val family = when (categoryOf(file.ext)) {
                Category.Docs -> OfficeFamily.DOCUMENT
                Category.Slides -> OfficeFamily.PRESENTATION
                Category.Sheets -> OfficeFamily.SPREADSHEET
                else -> error("Papercraft can't open .${file.ext} files as office files.")
            }
            return OfficePages(OfficeDoc.open(context.readBytes(file), family))
        }
    }
}
