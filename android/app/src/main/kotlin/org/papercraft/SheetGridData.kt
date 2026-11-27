package org.papercraft

import android.content.Context
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import android.text.Layout
import android.text.StaticLayout
import android.text.TextPaint
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.papercraft.core.OfficeFamily
import org.papercraft.core.SheetCell
import org.papercraft.core.SheetCellStyle
import org.papercraft.core.SheetDoc
import org.papercraft.core.SheetHAlign
import org.papercraft.core.SheetHyperlink
import org.papercraft.core.SheetLayout
import org.papercraft.core.SheetSizeRun
import org.papercraft.core.SheetTab
import org.papercraft.core.SheetVAlign
import org.papercraft.core.SheetValueKind
import kotlin.math.ceil

/** A workbook opened for the cell grid. Sheets load on first use and stay loaded. */
class SheetGridBook private constructor(private val doc: SheetDoc) {
    val tabs: List<SheetTab> = doc.sheets()
    val names = tabs.map { it.name }
    /** The sheet the file was saved on, or the first visible one when that is hidden. */
    val firstSheet: Int = doc.activeSheet().toInt().let { active ->
        if (tabs.getOrNull(active)?.hidden == false) active else tabs.indexOfFirst { !it.hidden }.coerceAtLeast(0)
    }
    private val sheets = HashMap<Int, GridSheet>()

    /** Blocking. */
    fun sheet(index: Int): GridSheet = synchronized(sheets) { sheets.getOrPut(index) { GridSheet.load(doc, index) } }

    /** Blocking: one window of cells, ready to draw. */
    internal fun cells(sheet: GridSheet, key: GridBlockKey, fonts: GridFonts): List<GridCellDraw> =
        doc.cells(sheet.index.toUInt(), key.firstRow.toUInt(), GRID_BLOCK_ROWS.toUInt(), key.firstCol.toUInt(), GRID_BLOCK_COLS.toUInt())
            .map { GridCellDraw.of(it, sheet, fonts) }

    /** Frees the file; calls still running finish first. */
    fun close() = doc.close()

    companion object {
        /** Blocking: reads the file and parses the sheet it was saved on. */
        fun open(context: Context, file: DocFile) = SheetGridBook(SheetDoc.open(context.readBytes(file), OfficeFamily.SPREADSHEET))
    }
}

/** One sheet's shape: track sizes, frozen panes, merges and links. */
class GridSheet private constructor(
    val index: Int,
    val layout: SheetLayout,
    val cols: GridAxis,
    val rows: GridAxis,
    val merges: GridMerges,
    val links: List<SheetHyperlink>,
) {
    /** Tracks the grid scrolls over: the used range, and the frozen panes even when blank. */
    val rowCount = maxOf(layout.rowCount.toInt(), layout.frozenRows.toInt())
    val colCount = maxOf(layout.colCount.toInt(), layout.frozenCols.toInt())

    companion object {
        fun load(doc: SheetDoc, index: Int): GridSheet {
            val sheet = index.toUInt()
            val layout = doc.layout(sheet)
            fun runs(list: List<SheetSizeRun>) = list.map { GridRun(it.first.toInt(), it.last.toInt(), it.px) }
            return GridSheet(
                index,
                layout,
                GridAxis(layout.defaultColWidthPx, runs(doc.colWidths(sheet))),
                GridAxis(layout.defaultRowHeightPx, runs(doc.rowHeights(sheet))),
                GridMerges(doc.merges(sheet).map { GridRect(it.firstRow.toInt(), it.firstCol.toInt(), it.lastRow.toInt(), it.lastCol.toInt()) }),
                doc.hyperlinks(sheet),
            )
        }
    }
}

/** Sheet pixels between a cell's edge and its text, as Excel leaves. */
internal const val GRID_TEXT_PAD = 3f

/** A cell prepared off the main thread: its paint, measured text and, for wrapped text, its laid-out lines. */
internal class GridCellDraw(
    val row: Int,
    val col: Int,
    val style: SheetCellStyle,
    /** The merge this cell anchors or sits in, or null. */
    val merge: GridRect?,
    /** One line of text, newlines as spaces; empty when nothing to write. */
    val text: String,
    val number: Boolean,
    val paint: TextPaint?,
    val textWidth: Float,
    /** Lines of wrapped text, laid out to the cell's (or merge's) width. */
    val lines: StaticLayout?,
) {
    companion object {
        fun of(cell: SheetCell, sheet: GridSheet, fonts: GridFonts): GridCellDraw {
            val row = cell.row.toInt()
            val col = cell.col.toInt()
            val style = cell.style
            val merge = sheet.merges.at(row, col)
            if (cell.text.isEmpty() || cell.kind == SheetValueKind.EMPTY) {
                return GridCellDraw(row, col, style, merge, "", false, null, 0f, null)
            }
            val paint = fonts.paint(style)
            val wraps = style.wrap || style.hAlign == SheetHAlign.JUSTIFY || style.hAlign == SheetHAlign.DISTRIBUTED ||
                style.vAlign == SheetVAlign.JUSTIFY || style.vAlign == SheetVAlign.DISTRIBUTED
            val lines = if (wraps) {
                val width = if (merge != null) sheet.cols.start(merge.lastCol + 1) - sheet.cols.start(merge.firstCol) else sheet.cols.size(col).toDouble()
                val inner = ceil(width - 2 * GRID_TEXT_PAD).toInt()
                if (inner > 0) {
                    val align = when (style.hAlign) {
                        SheetHAlign.CENTER, SheetHAlign.CENTER_ACROSS -> Layout.Alignment.ALIGN_CENTER
                        SheetHAlign.RIGHT -> Layout.Alignment.ALIGN_OPPOSITE
                        else -> Layout.Alignment.ALIGN_NORMAL
                    }
                    StaticLayout.Builder.obtain(cell.text, 0, cell.text.length, paint, inner).setAlignment(align).setIncludePad(false).build()
                } else null
            } else null
            val text = if (lines == null) cell.text.replace('\n', ' ').replace("\r", "") else cell.text
            val width = if (lines == null) paint.measureText(text) else 0f
            return GridCellDraw(row, col, style, merge, text, cell.kind == SheetValueKind.NUMBER, paint, width, lines)
        }
    }
}

/** Text paints by font, shared by the cells that use them. Paint sizes are in sheet pixels. */
internal class GridFonts {
    private data class Key(val family: Typeface, val px: Float, val bold: Boolean, val italic: Boolean, val underline: Boolean, val strike: Boolean, val color: Int)

    private val paints = HashMap<Key, TextPaint>()

    @Synchronized
    fun paint(style: SheetCellStyle): TextPaint {
        val key = Key(
            familyOf(style.fontName), style.fontSizePt * 96f / 72f, style.bold, style.italic, style.underline, style.strike,
            style.fontColor?.toInt() ?: Color.BLACK,
        )
        return paints.getOrPut(key) {
            TextPaint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG or Paint.LINEAR_TEXT_FLAG).apply {
                val weight = when {
                    key.bold && key.italic -> Typeface.BOLD_ITALIC
                    key.bold -> Typeface.BOLD
                    key.italic -> Typeface.ITALIC
                    else -> Typeface.NORMAL
                }
                typeface = Typeface.create(key.family, weight)
                textSize = key.px
                color = key.color
                isUnderlineText = key.underline
                isStrikeThruText = key.strike
            }
        }
    }

    // Limit: fonts map to the phone's sans, serif or mono family by name; bundling metric-compatible fonts (Carlito, Liberation) lifts it.
    private fun familyOf(name: String?): Typeface {
        val n = name?.lowercase().orEmpty()
        return when {
            listOf("courier", "consolas", "mono", "lucida console").any { it in n } -> Typeface.MONOSPACE
            "sans" in n -> Typeface.SANS_SERIF
            listOf("times", "cambria", "georgia", "garamond", "palatino", "book antiqua", "serif", "song", "ming").any { it in n } -> Typeface.SERIF
            else -> Typeface.SANS_SERIF
        }
    }
}

/** A loaded block of cells, indexed by position. */
internal class GridBlock(val cells: List<GridCellDraw>, private val key: GridBlockKey) {
    private val byPos = arrayOfNulls<GridCellDraw>(GRID_BLOCK_ROWS * GRID_BLOCK_COLS).also { grid ->
        for (cell in cells) grid[(cell.row - key.firstRow) * GRID_BLOCK_COLS + (cell.col - key.firstCol)] = cell
    }

    fun at(row: Int, col: Int): GridCellDraw? = byPos[(row - key.firstRow) * GRID_BLOCK_COLS + (col - key.firstCol)]
}

/** Blocks kept per sheet: enough for a tablet screen at the smallest zoom plus the way back. */
private const val GRID_CACHE_BLOCKS = 96

/**
 * The cells of one sheet, loaded block by block as they scroll into view. Only the main thread touches
 * the cache; [version] changes whenever a block arrives, so reading it redraws the grid.
 */
internal class GridCells(private val book: SheetGridBook, private val sheet: GridSheet) {
    private val fonts = GridFonts()
    private val blocks = object : LinkedHashMap<GridBlockKey, GridBlock>(GRID_CACHE_BLOCKS, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<GridBlockKey, GridBlock>) = size > GRID_CACHE_BLOCKS
    }
    var version by mutableIntStateOf(0)
        private set

    operator fun get(key: GridBlockKey): GridBlock? = blocks[key]

    fun cell(row: Int, col: Int): GridCellDraw? = blocks[GridBlockKey(row / GRID_BLOCK_ROWS, col / GRID_BLOCK_COLS)]?.at(row, col)

    fun loaded(keys: Collection<GridBlockKey>) = keys.all { it in blocks }

    /** Loads the blocks not cached yet, one at a time off the main thread. A block that fails stays empty. */
    suspend fun load(keys: Collection<GridBlockKey>) {
        for (key in keys) {
            if (key in blocks) continue
            // One block at a time: a load the user scrolled past may still be running when the next starts.
            val cells = withContext(Dispatchers.IO) { synchronized(fonts) { runCatching { book.cells(sheet, key, fonts) }.getOrDefault(emptyList()) } }
            blocks[key] = GridBlock(cells, key)
            version++
        }
    }
}
