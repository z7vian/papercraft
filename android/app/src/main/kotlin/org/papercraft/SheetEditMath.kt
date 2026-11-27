package org.papercraft

import org.papercraft.core.SheetCellChange
import org.papercraft.core.SheetCellRange
import org.papercraft.core.SheetValueKind
import kotlin.math.max
import kotlin.math.min

/** Where Enter (Down), Shift+Enter (Up), Tab (Right) and Shift+Tab (Left) move the active cell. */
internal enum class SheetMove { Down, Up, Right, Left }

/**
 * The selected cells: [range], and inside it the active cell ([row], [col]) the formula bar edits. A merged active cell
 * is its merge's top-left cell. [tabCol] is the column a run of Tab presses started from: Enter goes back to it, as in Excel.
 */
internal data class SheetSelection(val row: Int, val col: Int, val range: GridRect, val tabCol: Int? = null) {
    val rowCount get() = range.lastRow - range.firstRow + 1
    val colCount get() = range.lastCol - range.firstCol + 1

    /** The cells the active cell covers: its merge, or itself. */
    fun active(merges: GridMerges): GridRect = merges.at(row, col) ?: GridRect(row, col, row, col)

    /** True when nothing but the active cell is selected. */
    fun isSingle(merges: GridMerges) = range == active(merges)

    /** The block from the active cell to ([toRow], [toCol]), grown to cover every merge it cuts. */
    fun extendTo(toRow: Int, toCol: Int, merges: GridMerges): SheetSelection {
        val a = active(merges)
        val block = GridRect(min(a.firstRow, toRow), min(a.firstCol, toCol), max(a.lastRow, toRow), max(a.lastCol, toCol))
        return copy(range = coverMerges(block, merges), tabCol = null)
    }

    /**
     * Where the active cell goes after Enter or Tab. A single cell moves to its neighbour (Enter after Tabs returns to the
     * column the Tabs started from); in a block the active cell walks through the block and wraps, and the block stays.
     */
    fun moved(move: SheetMove, merges: GridMerges): SheetSelection {
        if (isSingle(merges)) {
            val a = active(merges)
            return when (move) {
                SheetMove.Down -> at(min(a.lastRow + 1, SHEET_ROWS - 1), tabCol ?: col, merges)
                SheetMove.Up -> at(max(a.firstRow - 1, 0), tabCol ?: col, merges)
                SheetMove.Right -> at(row, min(a.lastCol + 1, SHEET_COLS - 1), merges).copy(tabCol = tabCol ?: col)
                SheetMove.Left -> at(row, max(a.firstCol - 1, 0), merges).copy(tabCol = tabCol ?: col)
            }
        }
        var r = row
        var c = col
        // Every cell of the block is tried at most once; cells inside a merge but not its top-left are skipped.
        repeat(rowCount * colCount) {
            when (move) {
                SheetMove.Down -> if (++r > range.lastRow) { r = range.firstRow; if (++c > range.lastCol) c = range.firstCol }
                SheetMove.Up -> if (--r < range.firstRow) { r = range.lastRow; if (--c < range.firstCol) c = range.lastCol }
                SheetMove.Right -> if (++c > range.lastCol) { c = range.firstCol; if (++r > range.lastRow) r = range.firstRow }
                SheetMove.Left -> if (--c < range.firstCol) { c = range.lastCol; if (--r < range.firstRow) r = range.lastRow }
            }
            val merge = merges.at(r, c)
            if (merge == null || (merge.firstRow == r && merge.firstCol == c)) return SheetSelection(r, c, range)
        }
        return this
    }

    /** The selection as the sheet editor takes ranges. */
    fun cellRange() = SheetCellRange(range.firstRow.toUInt(), range.firstCol.toUInt(), range.lastRow.toUInt(), range.lastCol.toUInt())

    companion object {
        /** The cell at ([row], [col]) alone, or its whole merge. */
        fun at(row: Int, col: Int, merges: GridMerges): SheetSelection {
            val merge = merges.at(row, col) ?: return SheetSelection(row, col, GridRect(row, col, row, col))
            return SheetSelection(merge.firstRow, merge.firstCol, merge)
        }
    }
}

/** [block] grown until no merge sticks out of it. */
internal fun coverMerges(block: GridRect, merges: GridMerges): GridRect {
    if (merges.isEmpty) return block
    var r = block
    val found = ArrayList<GridRect>()
    while (true) {
        found.clear()
        merges.inWindow(r.firstRow..r.lastRow, r.firstCol..r.lastCol, found)
        var grown = r
        for (m in found) {
            grown = GridRect(min(grown.firstRow, m.firstRow), min(grown.firstCol, m.firstCol), max(grown.lastRow, m.lastRow), max(grown.lastCol, m.lastCol))
        }
        if (grown == r) return r
        r = grown
    }
}

/** `B7` for row 6, column 1 (both 0-based). */
internal fun sheetCellName(row: Int, col: Int) = sheetColumnName(col) + (row + 1)

/** What the name box shows: the active cell, or the block as `B2:D9`. */
internal fun selectionName(selection: SheetSelection, merges: GridMerges): String {
    if (selection.isSingle(merges)) return sheetCellName(selection.row, selection.col)
    val r = selection.range
    return sheetCellName(r.firstRow, r.firstCol) + ":" + sheetCellName(r.lastRow, r.lastCol)
}

/** A cell of a sheet, 0-based. */
internal data class SheetCellAt(val row: Int, val col: Int)

/**
 * Cell text the editor changed that the shown workbook doesn't hold yet. Typing shows from here at once; a reload of the
 * workbook after edit [SheetOverlay.shown] takes over the entries up to that edit.
 */
internal class SheetOverlay {
    private class Entry(val display: String, val edit: Int)

    private val sheets = HashMap<Int, HashMap<SheetCellAt, Entry>>()

    /** Records [changes] made by edit number [edit]; a later change to a cell replaces an earlier one. */
    fun add(changes: List<SheetCellChange>, edit: Int) {
        for (c in changes) sheets.getOrPut(c.sheet.toInt()) { HashMap() }[SheetCellAt(c.row.toInt(), c.col.toInt())] = Entry(c.display, edit)
    }

    /** The cells of [sheet] to redraw, with their new text. */
    fun of(sheet: Int): Map<SheetCellAt, String> = sheets[sheet]?.mapValues { it.value.display }.orEmpty()

    /** The shown workbook now holds every edit up to [edit]. */
    fun shown(edit: Int) {
        for (cells in sheets.values) cells.values.removeAll { it.edit <= edit }
        sheets.values.removeAll { it.isEmpty() }
    }
}

/**
 * The kind of value [display] text shows, for drawing it before the reload brings the real one: numbers, dates and
 * times (digits and no letters, bar an exponent) align right, TRUE/FALSE and errors centre.
 */
internal fun displayKind(display: String): SheetValueKind {
    val t = display.trim()
    return when {
        t.isEmpty() -> SheetValueKind.EMPTY
        t == "TRUE" || t == "FALSE" -> SheetValueKind.BOOL
        t.startsWith("#") && (t.endsWith("!") || t.endsWith("?") || t == "#N/A") -> SheetValueKind.ERROR
        t.any { it.isDigit() } && t.replace(EXPONENT, "").none { it.isLetter() } -> SheetValueKind.NUMBER
        else -> SheetValueKind.TEXT
    }
}

private val EXPONENT = Regex("(?<=\\d)E[+-]?(?=\\d)")

/** `Sheet4` for a book with Sheet1 to Sheet3: the first such name not taken, ignoring case as Excel does. */
internal fun newSheetName(names: List<String>): String {
    val taken = names.map { it.lowercase() }.toSet()
    return generateSequence(1) { it + 1 }.map { "Sheet$it" }.first { it.lowercase() !in taken }
}

/** Why Excel would refuse [name] for sheet [self] among [names], or null when it's fine. */
internal fun sheetNameProblem(name: String, names: List<String>, self: Int?): String? = when {
    name.isBlank() -> "Type a name."
    name.length > 31 -> "Sheet names are at most 31 characters."
    name.any { it in ":\\/?*[]" } -> "Sheet names can't contain : \\ / ? * [ or ]."
    name.startsWith("'") || name.endsWith("'") -> "Sheet names can't start or end with '."
    names.withIndex().any { (i, n) -> i != self && n.equals(name, ignoreCase = true) } -> "Another sheet already has that name."
    else -> null
}

/**
 * The scroll (sheet pixels) that brings a track from [start] to [end] into a [span] wide view now at [scroll]: unchanged
 * when it shows already, else just far enough; a track wider than the view shows its start.
 */
internal fun revealScroll(scroll: Double, start: Double, end: Double, span: Double): Double = when {
    start < scroll -> start
    end > scroll + span -> min(start, end - span)
    else -> scroll
}
