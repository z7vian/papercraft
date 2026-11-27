package org.papercraft

import android.graphics.Canvas
import android.graphics.DashPathEffect
import android.graphics.Paint
import android.text.TextPaint
import org.papercraft.core.SheetBorder
import org.papercraft.core.SheetHAlign
import org.papercraft.core.SheetLineStyle
import org.papercraft.core.SheetVAlign
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min

/** Colours of the grid's chrome (ARGB); cells bring their own. */
internal class GridColors(
    val paper: Int,
    val gridline: Int,
    val headerBg: Int,
    val headerText: Int,
    val headerLine: Int,
    val frozenLine: Int,
)

/** Columns beside the screen whose text may run into it; their blocks load with the visible ones. */
// Limit: text running in from more than this many columns off screen is cut at the screen edge; widening it loads more blocks.
internal const val GRID_SPILL_COLS = 4

/** A long run of '#', the first n of which stand for a number too wide for its column, as in Excel. */
private val HASHES = "#".repeat(256)

/**
 * Draws a sheet onto an Android canvas: frozen panes, the scrolling body and the headers. Cells draw
 * in sheet pixels under the canvas's scale, so zooming re-lays out nothing. Main thread only.
 */
internal class GridPainter(
    private val sheet: GridSheet,
    private val cells: GridCells,
    private val colors: GridColors,
    private val headerTextPx: Float,
) {
    private val fill = Paint().apply { style = Paint.Style.FILL }
    private val line = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
    private val headerPaint = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        textAlign = Paint.Align.CENTER
        color = colors.headerText
    }
    private val label = CharArray(12)
    private val merges = ArrayList<GridRect>()
    private val dashes = HashMap<SheetLineStyle, DashPathEffect>()

    // The region being drawn: its sheet-pixel origin and the screen scale.
    private var ox = 0.0
    private var oy = 0.0
    private var scale = 1f

    private fun xOf(col: Int) = (sheet.cols.start(col) - ox).toFloat()
    private fun yOf(row: Int) = (sheet.rows.start(row) - oy).toFloat()

    fun draw(c: Canvas, v: GridViewport) {
        fill.color = colors.paper
        c.drawRect(0f, 0f, v.width, v.height, fill)
        val fc = v.frozenCols
        val fr = v.frozenRows
        val frozenCols = 0 until fc
        val frozenRows = 0 until fr
        val bodyX0 = sheet.cols.start(fc) + v.scrollX
        val bodyY0 = sheet.rows.start(fr) + v.scrollY
        region(c, v, frozenRows, frozenCols, v.headerW, v.headerH, v.bodyX, v.bodyY, 0.0, 0.0, 0)
        region(c, v, frozenRows, v.bodyCols, v.bodyX, v.headerH, v.width, v.bodyY, bodyX0, 0.0, fc)
        region(c, v, v.bodyRows, frozenCols, v.headerW, v.bodyY, v.bodyX, v.height, 0.0, bodyY0, 0)
        region(c, v, v.bodyRows, v.bodyCols, v.bodyX, v.bodyY, v.width, v.height, bodyX0, bodyY0, fc)

        line.pathEffect = null
        line.color = colors.frozenLine
        line.strokeWidth = max(1f, v.scale)
        if (fc > 0) c.drawLine(v.bodyX, v.headerH, v.bodyX, v.height, line)
        if (fr > 0) c.drawLine(v.headerW, v.bodyY, v.width, v.bodyY, line)
        headers(c, v, bodyX0, bodyY0)
    }

    /** One pane: [rows] x [cols] drawn in the screen box, its top-left showing sheet point ([x0], [y0]). */
    private fun region(
        c: Canvas, v: GridViewport, rows: IntRange, cols: IntRange,
        left: Float, top: Float, right: Float, bottom: Float, x0: Double, y0: Double, domainFirst: Int,
    ) {
        if (rows.isEmpty() || cols.isEmpty() || right <= left || bottom <= top) return
        ox = x0
        oy = y0
        scale = v.scale
        // Text from just beside the pane may run into it, but not across the frozen line.
        val colLo = max(domainFirst, cols.first - GRID_SPILL_COLS)
        val colHi = if (domainFirst == 0 && v.frozenCols > 0 && cols.last < v.frozenCols) v.frozenCols - 1 else cols.last + GRID_SPILL_COLS
        c.save()
        c.clipRect(left, top, right, bottom)
        c.translate(left, top)
        c.scale(scale, scale)

        if (sheet.layout.showGridlines) gridlines(c, rows, cols)
        merges.clear()
        sheet.merges.inWindow(rows, cols, merges)
        fill.color = colors.paper
        val inset = 0.5f / scale
        for (m in merges) c.drawRect(xOf(m.firstCol) + inset, yOf(m.firstRow) + inset, xOf(m.lastCol + 1) - inset, yOf(m.lastRow + 1) - inset, fill)
        for (m in merges) {
            val fillColor = cells.cell(m.firstRow, m.firstCol)?.style?.fill ?: continue
            fill.color = fillColor.toInt()
            c.drawRect(xOf(m.firstCol), yOf(m.firstRow), xOf(m.lastCol + 1), yOf(m.lastRow + 1), fill)
        }
        forCells(rows, cols.first, cols.last) { cell ->
            val color = cell.style.fill
            if (cell.merge == null && color != null) {
                fill.color = color.toInt()
                c.drawRect(xOf(cell.col), yOf(cell.row), xOf(cell.col + 1), yOf(cell.row + 1), fill)
            }
        }
        for (m in merges) cells.cell(m.firstRow, m.firstCol)?.let { text(c, it, colLo, colHi) }
        forCells(rows, colLo, colHi) { cell -> if (cell.merge == null) text(c, cell, colLo, colHi) }
        forCells(rows, cols.first, cols.last) { cell -> borders(c, cell) }
        c.restore()
    }

    private inline fun forCells(rows: IntRange, colLo: Int, colHi: Int, each: (GridCellDraw) -> Unit) {
        for (br in rows.first / GRID_BLOCK_ROWS..rows.last / GRID_BLOCK_ROWS) {
            for (bc in colLo / GRID_BLOCK_COLS..colHi / GRID_BLOCK_COLS) {
                val block = cells[GridBlockKey(br, bc)] ?: continue
                for (cell in block.cells) {
                    if (cell.row in rows && cell.col in colLo..colHi) each(cell)
                }
            }
        }
    }

    private fun gridlines(c: Canvas, rows: IntRange, cols: IntRange) {
        line.pathEffect = null
        line.color = colors.gridline
        line.strokeWidth = 0f // one screen pixel at any zoom
        val top = yOf(rows.first)
        val bottom = yOf(rows.last + 1)
        val left = xOf(cols.first)
        val right = xOf(cols.last + 1)
        for (col in cols.first..cols.last + 1) {
            val x = xOf(col)
            c.drawLine(x, top, x, bottom, line)
        }
        for (row in rows.first..rows.last + 1) {
            val y = yOf(row)
            c.drawLine(left, y, right, y, line)
        }
    }

    private fun text(c: Canvas, cell: GridCellDraw, colLo: Int, colHi: Int) {
        val paint = cell.paint ?: return
        val m = cell.merge
        val l = xOf(m?.firstCol ?: cell.col)
        val r = xOf((m?.lastCol ?: cell.col) + 1)
        val t = yOf(m?.firstRow ?: cell.row)
        val b = yOf((m?.lastRow ?: cell.row) + 1)
        if (r <= l || b <= t) return
        val style = cell.style
        val lines = cell.lines
        if (lines != null) {
            val h = lines.height.toFloat()
            val y = when (style.vAlign) {
                SheetVAlign.CENTER -> (t + b - h) / 2
                SheetVAlign.BOTTOM -> b - 1 - h
                else -> t + 1
            }
            c.save()
            c.clipRect(l, t, r, b)
            c.translate(l + GRID_TEXT_PAD, y)
            lines.draw(c)
            c.restore()
            return
        }
        val inner = r - l - 2 * GRID_TEXT_PAD
        val ascent = paint.ascent()
        val descent = paint.descent()
        val y = when (style.vAlign) {
            SheetVAlign.TOP -> t + 1 - ascent
            SheetVAlign.CENTER -> (t + b - ascent - descent) / 2
            else -> b - 1 - descent
        }
        c.save()
        if (cell.number && cell.textWidth > inner) {
            val count = min(HASHES.length, floor(inner / paint.measureText("#")).toInt())
            c.clipRect(l, t, r, b)
            if (count > 0) c.drawText(HASHES, 0, count, l + GRID_TEXT_PAD, y, paint)
            c.restore()
            return
        }
        var clipL = l
        var clipR = r
        val align = style.hAlign
        val w = cell.textWidth
        if (m == null && w > inner && align != SheetHAlign.FILL) {
            val way = when (align) {
                SheetHAlign.RIGHT -> GridSpill.LEFT
                SheetHAlign.CENTER -> GridSpill.BOTH
                else -> GridSpill.RIGHT
            }
            val row = cell.row
            val span = sheet.cols.spill(cell.col, (w + 2 * GRID_TEXT_PAD).toDouble(), way, colLo, colHi) { col ->
                cells.cell(row, col)?.text.isNullOrEmpty() && sheet.merges.at(row, col) == null
            }
            clipL = xOf(span.first)
            clipR = xOf(span.last + 1)
        }
        c.clipRect(clipL, t, clipR, b)
        when (align) {
            SheetHAlign.FILL -> {
                if (w > 0f) {
                    val count = max(1, floor(inner / w).toInt())
                    for (i in 0 until count) c.drawText(cell.text, l + GRID_TEXT_PAD + i * w, y, paint)
                }
            }
            SheetHAlign.RIGHT -> c.drawText(cell.text, r - GRID_TEXT_PAD - w, y, paint)
            SheetHAlign.CENTER -> c.drawText(cell.text, (l + r - w) / 2, y, paint)
            SheetHAlign.CENTER_ACROSS -> c.drawText(cell.text, (clipL + clipR - w) / 2, y, paint)
            else -> c.drawText(cell.text, l + GRID_TEXT_PAD, y, paint)
        }
        c.restore()
    }

    /** A cell's sides; inside a merge only the sides on the merge's edge show. */
    private fun borders(c: Canvas, cell: GridCellDraw) {
        val s = cell.style
        if (s.borderLeft == null && s.borderRight == null && s.borderTop == null && s.borderBottom == null) return
        val m = cell.merge
        val l = xOf(cell.col)
        val r = xOf(cell.col + 1)
        val t = yOf(cell.row)
        val b = yOf(cell.row + 1)
        if (m == null || cell.col == m.firstCol) edge(c, s.borderLeft, l, t, l, b)
        if (m == null || cell.col == m.lastCol) edge(c, s.borderRight, r, t, r, b)
        if (m == null || cell.row == m.firstRow) edge(c, s.borderTop, l, t, r, t)
        if (m == null || cell.row == m.lastRow) edge(c, s.borderBottom, l, b, r, b)
    }

    private fun edge(c: Canvas, border: SheetBorder?, x1: Float, y1: Float, x2: Float, y2: Float) {
        border ?: return
        val style = border.style
        line.color = border.color.toInt()
        line.pathEffect = dash(style)
        val width = when (style) {
            SheetLineStyle.HAIR -> 0f
            SheetLineStyle.MEDIUM, SheetLineStyle.MEDIUM_DASHED, SheetLineStyle.MEDIUM_DASH_DOT,
            SheetLineStyle.MEDIUM_DASH_DOT_DOT, SheetLineStyle.SLANT_DASH_DOT -> 2f
            SheetLineStyle.THICK -> 3f
            else -> 1f
        }
        line.strokeWidth = if (width == 0f) 0f else max(width, 1f / scale)
        if (style == SheetLineStyle.DOUBLE) {
            // Two thin lines either side of the edge.
            val dx = if (x1 == x2) 1f else 0f
            val dy = if (y1 == y2) 1f else 0f
            c.drawLine(x1 - dx, y1 - dy, x2 - dx, y2 - dy, line)
            c.drawLine(x1 + dx, y1 + dy, x2 + dx, y2 + dy, line)
        } else {
            c.drawLine(x1, y1, x2, y2, line)
        }
    }

    /** Excel's dash patterns, in sheet pixels. */
    private fun dash(style: SheetLineStyle): DashPathEffect? {
        val intervals = when (style) {
            SheetLineStyle.DASHED -> floatArrayOf(3f, 1f)
            SheetLineStyle.DOTTED -> floatArrayOf(1f, 1f)
            SheetLineStyle.MEDIUM_DASHED -> floatArrayOf(9f, 3f)
            SheetLineStyle.DASH_DOT, SheetLineStyle.MEDIUM_DASH_DOT -> floatArrayOf(9f, 3f, 3f, 3f)
            SheetLineStyle.DASH_DOT_DOT, SheetLineStyle.MEDIUM_DASH_DOT_DOT -> floatArrayOf(9f, 3f, 3f, 3f, 3f, 3f)
            SheetLineStyle.SLANT_DASH_DOT -> floatArrayOf(11f, 1f, 5f, 1f)
            else -> return null
        }
        return dashes.getOrPut(style) { DashPathEffect(intervals, 0f) }
    }

    /** Column letters and row numbers, in screen pixels, over the panes they label. */
    private fun headers(c: Canvas, v: GridViewport, bodyX0: Double, bodyY0: Double) {
        val s = v.scale
        val cols = sheet.cols
        val rows = sheet.rows
        fill.color = colors.headerBg
        c.drawRect(0f, 0f, v.width, v.headerH, fill)
        c.drawRect(0f, 0f, v.headerW, v.height, fill)
        line.pathEffect = null
        line.color = colors.headerLine
        line.strokeWidth = 0f
        headerPaint.textSize = min(headerTextPx, (sheet.layout.defaultRowHeightPx * s * 0.75f).coerceAtLeast(headerTextPx * 0.6f))
        val ascent = headerPaint.ascent()
        val descent = headerPaint.descent()

        val colBaseline = (v.headerH - ascent - descent) / 2
        c.save()
        c.clipRect(v.headerW, 0f, v.width, v.headerH)
        for (col in 0 until v.frozenCols) colLabel(c, col, v.headerW + (cols.start(col) * s).toFloat(), cols.size(col) * s, v.headerH, colBaseline)
        c.clipRect(v.bodyX, 0f, v.width, v.headerH)
        for (col in v.bodyCols) colLabel(c, col, v.bodyX + ((cols.start(col) - bodyX0) * s).toFloat(), cols.size(col) * s, v.headerH, colBaseline)
        c.restore()

        val textMid = -(ascent + descent) / 2
        c.save()
        c.clipRect(0f, v.headerH, v.headerW, v.height)
        for (row in 0 until v.frozenRows) rowLabel(c, row, v.headerH + (rows.start(row) * s).toFloat(), rows.size(row) * s, v.headerW, textMid)
        c.clipRect(0f, v.bodyY, v.headerW, v.height)
        for (row in v.bodyRows) rowLabel(c, row, v.bodyY + ((rows.start(row) - bodyY0) * s).toFloat(), rows.size(row) * s, v.headerW, textMid)
        c.restore()

        c.drawLine(0f, v.headerH, v.width, v.headerH, line)
        c.drawLine(v.headerW, 0f, v.headerW, v.height, line)
    }

    private fun colLabel(c: Canvas, col: Int, x: Float, w: Float, h: Float, baseline: Float) {
        if (w < 1f) return
        var n = col + 1
        var at = label.size
        while (n > 0) {
            label[--at] = 'A' + (n - 1) % 26
            n = (n - 1) / 26
        }
        c.drawText(label, at, label.size - at, x + w / 2, baseline, headerPaint)
        c.drawLine(x + w, 0f, x + w, h, line)
    }

    private fun rowLabel(c: Canvas, row: Int, y: Float, h: Float, w: Float, textMid: Float) {
        if (h < 1f) return
        var n = row + 1
        var at = label.size
        while (n > 0) {
            label[--at] = '0' + n % 10
            n /= 10
        }
        c.drawText(label, at, label.size - at, w / 2, y + h / 2 + textMid, headerPaint)
        c.drawLine(0f, y + h, w, y + h, line)
    }
}
