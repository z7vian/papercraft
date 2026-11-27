package org.papercraft

import kotlin.math.floor
import kotlin.math.max

/** Tracks [first]..[last] (0-based rows or columns), each [px] sheet pixels (96 dpi); 0 is hidden. */
data class GridRun(val first: Int, val last: Int, val px: Float)

/**
 * Where each row or column of a sheet starts, in sheet pixels. Tracks no run lists are [defaultPx].
 * Lookups binary-search the runs, so a million-row sheet costs only the runs it lists.
 */
class GridAxis(private val defaultPx: Float, runs: List<GridRun>) {
    private val firsts: IntArray
    private val lasts: IntArray
    private val sizes: FloatArray
    /** Offset of each run's first track. */
    private val starts: DoubleArray

    init {
        val sorted = runs.filter { it.first <= it.last && it.first >= 0 }.sortedBy { it.first }
        firsts = IntArray(sorted.size) { sorted[it].first }
        lasts = IntArray(sorted.size) { sorted[it].last }
        sizes = FloatArray(sorted.size) { sorted[it].px.coerceAtLeast(0f) }
        starts = DoubleArray(sorted.size)
        var end = 0.0
        var next = 0
        for (k in sorted.indices) {
            starts[k] = end + (firsts[k] - next).toDouble() * defaultPx
            end = starts[k] + (lasts[k] - firsts[k] + 1).toDouble() * sizes[k]
            next = lasts[k] + 1
        }
    }

    fun size(index: Int): Float {
        val k = runAtOrBefore(index)
        return if (k >= 0 && index <= lasts[k]) sizes[k] else defaultPx
    }

    /** Offset of track [index]'s leading edge; `start(n)` is also the end of track `n - 1`. */
    fun start(index: Int): Double {
        val k = runAtOrBefore(index)
        if (k < 0) return index.toDouble() * defaultPx
        val f = firsts[k]
        val l = lasts[k]
        return if (index <= l) starts[k] + (index - f).toDouble() * sizes[k]
        else starts[k] + (l - f + 1).toDouble() * sizes[k] + (index - l - 1).toDouble() * defaultPx
    }

    /** The track holding [offset]; a boundary belongs to the track after it, and hidden tracks hold no offset. */
    fun indexAt(offset: Double): Int {
        val at = max(offset, 0.0)
        val k = startAtOrBefore(at)
        if (k < 0) return if (defaultPx > 0f) floor(at / defaultPx).toInt() else 0
        val runEnd = starts[k] + (lasts[k] - firsts[k] + 1).toDouble() * sizes[k]
        if (at < runEnd) return (firsts[k] + floor((at - starts[k]) / sizes[k]).toInt()).coerceAtMost(lasts[k])
        return if (defaultPx > 0f) lasts[k] + 1 + floor((at - runEnd) / defaultPx).toInt() else lasts[k] + 1
    }

    /**
     * Tracks of the scrolling part (from [frozen] on) seen when it is scrolled [scroll] sheet pixels and
     * [span] sheet pixels of it fit on screen.
     */
    fun visible(frozen: Int, scroll: Double, span: Double): IntRange {
        val origin = start(frozen) + max(scroll, 0.0)
        val first = max(frozen, indexAt(origin))
        val end = origin + max(span, 0.0)
        val past = indexAt(end)
        val last = if (past > first && start(past) >= end) past - 1 else past
        return first..max(first, last)
    }

    /** Last run whose first track is at or before [index], or -1. */
    private fun runAtOrBefore(index: Int): Int {
        var lo = 0
        var hi = firsts.size - 1
        var found = -1
        while (lo <= hi) {
            val mid = (lo + hi) ushr 1
            if (firsts[mid] <= index) { found = mid; lo = mid + 1 } else hi = mid - 1
        }
        return found
    }

    /** Last run starting at or before [offset], or -1. */
    private fun startAtOrBefore(offset: Double): Int {
        var lo = 0
        var hi = starts.size - 1
        var found = -1
        while (lo <= hi) {
            val mid = (lo + hi) ushr 1
            if (starts[mid] <= offset) { found = mid; lo = mid + 1 } else hi = mid - 1
        }
        return found
    }
}

/** A cell rectangle, 0-based and inclusive. */
data class GridRect(val firstRow: Int, val firstCol: Int, val lastRow: Int, val lastCol: Int) {
    fun contains(row: Int, col: Int) = row in firstRow..lastRow && col in firstCol..lastCol
    fun intersects(rows: IntRange, cols: IntRange) =
        firstRow <= rows.last && lastRow >= rows.first && firstCol <= cols.last && lastCol >= cols.first
}

/** A sheet's merged ranges, sorted by first row so a window finds its merges without scanning them all. */
class GridMerges(ranges: List<GridRect>) {
    private val sorted = ranges.sortedBy { it.firstRow }
    private val tallest = sorted.maxOfOrNull { it.lastRow - it.firstRow } ?: 0

    val isEmpty get() = sorted.isEmpty()

    /** The merge covering a cell, or null. */
    fun at(row: Int, col: Int): GridRect? {
        if (sorted.isEmpty()) return null
        for (k in firstCandidate(row) until sorted.size) {
            val merge = sorted[k]
            if (merge.firstRow > row) break
            if (merge.contains(row, col)) return merge
        }
        return null
    }

    /** Adds every merge that overlaps the window to [into]. */
    fun inWindow(rows: IntRange, cols: IntRange, into: MutableList<GridRect>) {
        if (sorted.isEmpty() || rows.isEmpty() || cols.isEmpty()) return
        for (k in firstCandidate(rows.first) until sorted.size) {
            val merge = sorted[k]
            if (merge.firstRow > rows.last) break
            if (merge.intersects(rows, cols)) into.add(merge)
        }
    }

    /** First merge that could reach [row]: none starting more than [tallest] rows above it can. */
    private fun firstCandidate(row: Int): Int {
        val from = row - tallest
        var lo = 0
        var hi = sorted.size
        while (lo < hi) {
            val mid = (lo + hi) ushr 1
            if (sorted[mid].firstRow < from) lo = mid + 1 else hi = mid
        }
        return lo
    }
}

/** Cells load in blocks of this many rows and columns, so a scroll asks the core for a few windows, not cell by cell. */
const val GRID_BLOCK_ROWS = 64
const val GRID_BLOCK_COLS = 16

data class GridBlockKey(val row: Int, val col: Int) {
    val firstRow get() = row * GRID_BLOCK_ROWS
    val firstCol get() = col * GRID_BLOCK_COLS
}

/** The blocks covering every pair of a row range and a column range. */
fun gridBlocks(rowRanges: List<IntRange>, colRanges: List<IntRange>): Set<GridBlockKey> {
    val keys = LinkedHashSet<GridBlockKey>()
    for (rows in rowRanges) {
        if (rows.isEmpty() || rows.last < 0) continue
        for (cols in colRanges) {
            if (cols.isEmpty() || cols.last < 0) continue
            for (r in max(rows.first, 0) / GRID_BLOCK_ROWS..rows.last / GRID_BLOCK_ROWS) {
                for (c in max(cols.first, 0) / GRID_BLOCK_COLS..cols.last / GRID_BLOCK_COLS) keys.add(GridBlockKey(r, c))
            }
        }
    }
    return keys
}

/** Spreadsheet column letters: 0 is A, 25 is Z, 26 is AA. */
fun sheetColumnName(index: Int): String {
    val name = StringBuilder()
    var n = index + 1
    while (n > 0) {
        name.append('A' + (n - 1) % 26)
        n = (n - 1) / 26
    }
    return name.reverse().toString()
}

/** Excel's last row and column, 1-based. */
internal const val SHEET_ROWS = 1_048_576
internal const val SHEET_COLS = 16_384

/** Where tapping a link goes. */
sealed interface GridLinkTarget {
    /** A web or mail address to hand to another app. */
    data class Web(val uri: String) : GridLinkTarget

    /** A cell in this workbook; [sheet] null means the sheet the link is on. */
    data class Place(val sheet: String?, val row: Int, val col: Int) : GridLinkTarget
}

/**
 * A cell link's target: web and mail addresses open as they are, `#Sheet!A1` and the location
 * attribute jump inside the workbook. Null when there is nowhere to go (a file, a defined name).
 */
fun gridLinkTarget(url: String?, location: String?): GridLinkTarget? {
    val address = url?.trim().orEmpty()
    if (address.startsWith("#")) gridPlace(address.substring(1))?.let { return it }
    val scheme = address.substringBefore(':', "").lowercase()
    if (scheme == "http" || scheme == "https" || scheme == "mailto") return GridLinkTarget.Web(address)
    if (address.startsWith("www.", ignoreCase = true)) return GridLinkTarget.Web("https://$address")
    return location?.let(::gridPlace)
}

/** `Sheet1!A2`, `'Q3 ''final'''!$B$4:C9` or `A2`; the first cell of a range. */
// Limit: defined names and other workbooks' cells (`[Book.xlsx]Sheet1!A1`) don't resolve; reading the workbook's names lifts it.
private fun gridPlace(text: String): GridLinkTarget.Place? {
    val place = text.trim()
    if (place.isEmpty()) return null
    val sheet: String?
    val ref: String
    if (place.startsWith("'")) {
        var i = 1
        val name = StringBuilder()
        while (true) {
            if (i >= place.length) return null
            val ch = place[i]
            if (ch == '\'') {
                if (place.getOrNull(i + 1) == '\'') { name.append('\''); i += 2; continue }
                break
            }
            name.append(ch)
            i++
        }
        if (place.getOrNull(i + 1) != '!') return null
        sheet = name.toString()
        ref = place.substring(i + 2)
    } else {
        val bang = place.lastIndexOf('!')
        sheet = if (bang >= 0) place.substring(0, bang) else null
        ref = if (bang >= 0) place.substring(bang + 1) else place
    }
    if (sheet != null && (sheet.isEmpty() || sheet.contains('['))) return null
    val cell = ref.substringBefore(':').replace("$", "").trim().uppercase()
    val letters = cell.takeWhile { it in 'A'..'Z' }
    val digits = cell.substring(letters.length)
    if (letters.isEmpty() || letters.length > 3 || digits.isEmpty() || digits.length > 7 || !digits.all { it in '0'..'9' }) return null
    val row = digits.toInt()
    val col = letters.fold(0) { acc, ch -> acc * 26 + (ch - 'A' + 1) }
    if (row !in 1..SHEET_ROWS || col !in 1..SHEET_COLS) return null
    return GridLinkTarget.Place(sheet, row - 1, col - 1)
}

/** The sheet a link names; Excel matches sheet names regardless of case. */
fun gridSheetIndex(names: List<String>, name: String): Int? =
    names.indexOfFirst { it.equals(name, ignoreCase = true) }.takeIf { it >= 0 }

/** Which way non-wrapping text may run into empty neighbours: left-aligned text runs right, and so on. */
enum class GridSpill { RIGHT, LEFT, BOTH }

/**
 * Columns a non-wrapping text [need] sheet pixels wide covers from [col], running over [free]
 * neighbours within [minCol]..[maxCol], as Excel lets text run into empty cells.
 */
fun GridAxis.spill(col: Int, need: Double, way: GridSpill, minCol: Int, maxCol: Int, free: (Int) -> Boolean): IntRange {
    val extra = need - size(col)
    if (extra <= 0) return col..col
    val side = if (way == GridSpill.BOTH) extra / 2 else extra
    var first = col
    var last = col
    if (way != GridSpill.RIGHT) {
        var got = 0.0
        while (got < side && first - 1 >= minCol && free(first - 1)) { first--; got += size(first) }
    }
    if (way != GridSpill.LEFT) {
        var got = 0.0
        while (got < side && last + 1 <= maxCol && free(last + 1)) { last++; got += size(last) }
    }
    return first..last
}

/** The scroll that keeps the sheet point under [focusBefore]/[focusAfter] screen pixels put when the scale changes. */
fun gridZoomScroll(scroll: Double, focusBefore: Float, focusAfter: Float, scaleBefore: Float, scaleAfter: Float): Double =
    scroll + focusBefore / scaleBefore - focusAfter / scaleAfter

/** A freeze wider or taller than this share of the screen would leave nothing to scroll, so it is dropped, as Excel's phone app does. */
private const val MAX_FROZEN_SHARE = 0.6

/**
 * The grid on screen: headers, frozen panes and the scrolling body, in screen pixels.
 * [scale] is screen pixels per sheet pixel; [scrollX]/[scrollY] are sheet pixels past the frozen tracks.
 */
class GridViewport(
    val cols: GridAxis,
    val rows: GridAxis,
    frozenCols: Int,
    frozenRows: Int,
    val scale: Float,
    val scrollX: Double,
    val scrollY: Double,
    val width: Float,
    val height: Float,
    val headerW: Float,
    val headerH: Float,
) {
    val frozenCols = frozenCols.takeIf { it > 0 && cols.start(it) * scale <= (width - headerW) * MAX_FROZEN_SHARE } ?: 0
    val frozenRows = frozenRows.takeIf { it > 0 && rows.start(it) * scale <= (height - headerH) * MAX_FROZEN_SHARE } ?: 0
    val frozenW = (cols.start(this.frozenCols) * scale).toFloat()
    val frozenH = (rows.start(this.frozenRows) * scale).toFloat()
    /** Screen position of the scrolling body's top-left corner. */
    val bodyX = headerW + frozenW
    val bodyY = headerH + frozenH
    val bodyCols = cols.visible(this.frozenCols, scrollX, ((width - bodyX) / scale).toDouble())
    val bodyRows = rows.visible(this.frozenRows, scrollY, ((height - bodyY) / scale).toDouble())

    /** Furthest horizontal scroll that still shows column [count] - 1 at the right edge. */
    fun maxScrollX(count: Int): Double =
        max(0.0, cols.start(max(count, frozenCols)) - cols.start(frozenCols) - (width - bodyX) / scale)

    /** Furthest vertical scroll; [bottomPad] screen pixels at the bottom are covered by bars. */
    fun maxScrollY(count: Int, bottomPad: Float): Double =
        max(0.0, rows.start(max(count, frozenRows)) - rows.start(frozenRows) - (height - bodyY - bottomPad) / scale)

    /** Column under screen x, or null on the row headers. */
    fun colAt(x: Float): Int? = when {
        x < headerW -> null
        x < bodyX -> cols.indexAt(((x - headerW) / scale).toDouble()).coerceAtMost(frozenCols - 1)
        else -> cols.indexAt(cols.start(frozenCols) + scrollX + (x - bodyX) / scale)
    }

    /** Row under screen y, or null on the column headers. */
    fun rowAt(y: Float): Int? = when {
        y < headerH -> null
        y < bodyY -> rows.indexAt(((y - headerH) / scale).toDouble()).coerceAtMost(frozenRows - 1)
        else -> rows.indexAt(rows.start(frozenRows) + scrollY + (y - bodyY) / scale)
    }
}
