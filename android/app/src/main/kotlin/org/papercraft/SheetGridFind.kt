package org.papercraft

import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive

/** A cell holding a find match: sheet [sheet] (0 in a table), 0-based [row] and [col]. */
data class CellHit(val sheet: Int, val row: Int, val col: Int)

/**
 * Every cell matching a query across [sheets] (in tab order), at most [limit]: [search] lists one sheet's
 * matching cells row by row as (row, col), given how many more hits fit. Stops between sheets once cancelled.
 */
suspend fun findInSheets(sheets: List<Int>, limit: Int = MAX_FIND_HITS, search: (sheet: Int, limit: Int) -> List<Pair<Int, Int>>): List<CellHit> {
    val context = currentCoroutineContext()
    val hits = ArrayList<CellHit>()
    for (sheet in sheets) {
        context.ensureActive()
        val left = limit - hits.size
        if (left <= 0) break
        for ((row, col) in search(sheet, left).take(left)) hits += CellHit(sheet, row, col)
    }
    return hits
}

/**
 * Cells of [rows] (only the first [cols] columns, as the table shows) whose text holds [query], row by row,
 * at most [limit]. Case-insensitive like [findInLines].
 */
suspend fun findInTable(rows: List<List<String>>, cols: Int, query: String, limit: Int = MAX_FIND_HITS): List<CellHit> {
    if (query.isEmpty()) return emptyList()
    val context = currentCoroutineContext()
    val hits = ArrayList<CellHit>()
    for ((r, row) in rows.withIndex()) {
        context.ensureActive()
        for (c in 0 until minOf(cols, row.size)) {
            if (!row[c].contains(query, ignoreCase = true)) continue
            hits += CellHit(0, r, c)
            if (hits.size >= limit) return hits
        }
    }
    return hits
}

/** The first hit to show: the first on sheet [sheet] or a later tab, else the first of all; -1 when none. */
fun List<CellHit>.firstFromSheet(sheet: Int, tabs: List<Int>): Int {
    val rank = tabs.indexOf(sheet)
    return firstAtOrAfter(rank) { tabs.indexOf(it.sheet) }
}

/** The cells a match at [row], [col] fills on screen: its merge, or the cell alone. */
fun GridMerges.span(row: Int, col: Int): GridRect = at(row, col) ?: GridRect(row, col, row, col)

/**
 * The scroll that shows [start] until [end] in a window [span] long scrolled to [scroll]: unchanged when it
 * already shows, else just enough to bring it in, its start first when it doesn't fit.
 */
fun revealSpan(start: Double, end: Double, scroll: Double, span: Double): Double = when {
    start < scroll -> start
    end > scroll + span -> if (end - start > span) start else end - span
    else -> scroll
}

/**
 * The scroll (sheet pixels past [frozen] tracks) that shows tracks [first]..[last] of this axis when [span]
 * sheet pixels of the scrolling part fit on screen; unchanged for tracks in the frozen part, which always show.
 */
fun GridAxis.reveal(first: Int, last: Int, frozen: Int, scroll: Double, span: Double): Double {
    if (first < frozen) return scroll
    val origin = start(frozen)
    return revealSpan(start(first) - origin, start(last + 1) - origin, scroll, span)
}

/** A box on screen, in screen pixels. */
data class GridBox(val left: Float, val top: Float, val right: Float, val bottom: Float)

/**
 * Where [cells] shows on screen, cut to the pane its first cell sits in (frozen or scrolling) so it never
 * covers the headers or another pane; null when none of it shows.
 */
fun GridViewport.boxOf(cells: GridRect): GridBox? {
    val inFrozenCols = cells.firstCol < frozenCols
    val inFrozenRows = cells.firstRow < frozenRows
    // Screen position of a track's leading edge within the pane the cells sit in.
    fun x(col: Int): Float =
        if (inFrozenCols) headerW + (cols.start(col) * scale).toFloat()
        else bodyX + ((cols.start(col) - cols.start(frozenCols) - scrollX) * scale).toFloat()
    fun y(row: Int): Float =
        if (inFrozenRows) headerH + (rows.start(row) * scale).toFloat()
        else bodyY + ((rows.start(row) - rows.start(frozenRows) - scrollY) * scale).toFloat()
    val minX = if (inFrozenCols) headerW else bodyX
    val maxX = if (inFrozenCols) bodyX else width
    val minY = if (inFrozenRows) headerH else bodyY
    val maxY = if (inFrozenRows) bodyY else height
    val box = GridBox(
        x(cells.firstCol).coerceAtLeast(minX), y(cells.firstRow).coerceAtLeast(minY),
        x(cells.lastCol + 1).coerceAtMost(maxX), y(cells.lastRow + 1).coerceAtMost(maxY),
    )
    return box.takeIf { it.left < it.right && it.top < it.bottom }
}
