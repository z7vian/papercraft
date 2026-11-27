package org.papercraft

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.papercraft.core.SheetCellChange
import org.papercraft.core.SheetCellRange
import org.papercraft.core.SheetValueKind

class SheetEditTest {
    private val none = GridMerges(emptyList())

    // B2:C3 is merged.
    private val merged = GridMerges(listOf(GridRect(1, 1, 2, 2)))

    private fun cell(row: Int, col: Int, merges: GridMerges = none) = SheetSelection.at(row, col, merges)

    private fun change(sheet: Int, row: Int, col: Int, display: String) = SheetCellChange(sheet.toUInt(), row.toUInt(), col.toUInt(), display)

    @Test fun dragSelectsTheBlockBetweenTheActiveCellAndTheFinger() {
        val from = cell(3, 3)
        assertEquals(GridRect(3, 3, 5, 6), from.extendTo(5, 6, none).range)
        // Dragging up and left still gives a block with its first cell top-left; the active cell stays.
        val up = from.extendTo(1, 0, none)
        assertEquals(GridRect(1, 0, 3, 3), up.range)
        assertEquals(3 to 3, up.row to up.col)
        assertEquals(3, up.rowCount)
        assertEquals(4, up.colCount)
    }

    @Test fun blocksGrowToCoverTheMergesTheyCut() {
        assertEquals(GridRect(0, 0, 2, 2), cell(0, 0).extendTo(1, 1, merged).range)
        // A tap on any cell of a merge selects the whole merge, active at its top-left.
        val tapped = cell(2, 2, merged)
        assertEquals(1 to 1, tapped.row to tapped.col)
        assertEquals(GridRect(1, 1, 2, 2), tapped.range)
        assertTrue(tapped.isSingle(merged))
    }

    @Test fun selectionNamesItsCellOrBlockInA1Style() {
        assertEquals("A1", sheetCellName(0, 0))
        assertEquals("AB10", sheetCellName(9, 27))
        assertEquals("XFD1048576", sheetCellName(SHEET_ROWS - 1, SHEET_COLS - 1))
        assertEquals("C4", selectionName(cell(3, 2), none))
        assertEquals("B2", selectionName(cell(2, 2, merged), merged))
        assertEquals("A1:C3", selectionName(cell(0, 0).extendTo(1, 1, merged), merged))
        assertEquals(SheetCellRange(0u, 0u, 2u, 2u), cell(0, 0).extendTo(2, 2, none).cellRange())
    }

    @Test fun enterMovesDownAndTabMovesRight() {
        assertEquals(1 to 0, cell(0, 0).moved(SheetMove.Down, none).let { it.row to it.col })
        assertEquals(0 to 1, cell(0, 0).moved(SheetMove.Right, none).let { it.row to it.col })
        assertEquals(4 to 2, cell(5, 2).moved(SheetMove.Up, none).let { it.row to it.col })
        assertEquals(5 to 1, cell(5, 2).moved(SheetMove.Left, none).let { it.row to it.col })
    }

    @Test fun movesStopAtTheSheetsEdges() {
        assertEquals(cell(0, 0).range, cell(0, 0).moved(SheetMove.Up, none).range)
        assertEquals(cell(0, 0).range, cell(0, 0).moved(SheetMove.Left, none).range)
        val last = cell(SHEET_ROWS - 1, SHEET_COLS - 1)
        assertEquals(last.range, last.moved(SheetMove.Down, none).range)
        assertEquals(last.range, last.moved(SheetMove.Right, none).range)
    }

    @Test fun enterAfterTabsGoesBackToTheColumnTheyStartedFrom() {
        var s = cell(4, 1)
        s = s.moved(SheetMove.Right, none).moved(SheetMove.Right, none)
        assertEquals(4 to 3, s.row to s.col)
        s = s.moved(SheetMove.Down, none)
        assertEquals(5 to 1, s.row to s.col)
        assertNull(s.tabCol)
        // A tap starts afresh.
        assertNull(cell(4, 3).moved(SheetMove.Down, none).tabCol)
    }

    @Test fun movesStepOverMerges() {
        // Down from B1 lands on the merge B2:C3, then leaves it below.
        val onMerge = cell(0, 1).moved(SheetMove.Down, merged)
        assertEquals(GridRect(1, 1, 2, 2), onMerge.range)
        assertEquals(3 to 1, onMerge.moved(SheetMove.Down, merged).let { it.row to it.col })
        assertEquals(1 to 3, onMerge.moved(SheetMove.Right, merged).let { it.row to it.col })
    }

    @Test fun insideABlockEnterAndTabWalkThroughItAndWrap() {
        val block = cell(0, 0).extendTo(1, 1, none)
        val down = generateSequence(block) { it.moved(SheetMove.Down, none) }.take(5).map { it.row to it.col }.toList()
        assertEquals(listOf(0 to 0, 1 to 0, 0 to 1, 1 to 1, 0 to 0), down)
        val right = generateSequence(block) { it.moved(SheetMove.Right, none) }.take(5).map { it.row to it.col }.toList()
        assertEquals(listOf(0 to 0, 0 to 1, 1 to 0, 1 to 1, 0 to 0), right)
        assertEquals(1 to 1, block.moved(SheetMove.Up, none).let { it.row to it.col })
        assertEquals(1 to 1, block.moved(SheetMove.Left, none).let { it.row to it.col })
        assertEquals(block.range, block.moved(SheetMove.Down, none).range)
    }

    @Test fun walkingABlockSkipsCellsHiddenUnderAMerge() {
        val block = SheetSelection(0, 1, GridRect(0, 0, 2, 2))
        val first = block.moved(SheetMove.Down, merged)
        assertEquals(1 to 1, first.row to first.col)
        // B3 lies under the merge, so Enter goes on to the top of column C.
        assertEquals(0 to 2, first.moved(SheetMove.Down, merged).let { it.row to it.col })
    }

    @Test fun refreshTakesTheChangesOfTheShownSheetLastOneWinning() {
        val overlay = SheetOverlay()
        overlay.add(listOf(change(0, 0, 0, "5"), change(0, 3, 1, "15"), change(1, 0, 0, "other sheet")), edit = 1)
        overlay.add(listOf(change(0, 0, 0, "6"), change(0, 3, 1, "16")), edit = 2)
        assertEquals(mapOf(SheetCellAt(0, 0) to "6", SheetCellAt(3, 1) to "16"), overlay.of(0))
        assertEquals(mapOf(SheetCellAt(0, 0) to "other sheet"), overlay.of(1))
        assertEquals(emptyMap<SheetCellAt, String>(), overlay.of(2))
    }

    @Test fun aReloadTakesOverTheChangesItHolds() {
        val overlay = SheetOverlay()
        overlay.add(listOf(change(0, 0, 0, "a"), change(1, 0, 0, "b")), edit = 3)
        overlay.add(listOf(change(0, 1, 0, "c")), edit = 5)
        overlay.shown(4)
        assertEquals(mapOf(SheetCellAt(1, 0) to "c"), overlay.of(0))
        assertEquals(emptyMap<SheetCellAt, String>(), overlay.of(1))
        overlay.shown(5)
        assertEquals(emptyMap<SheetCellAt, String>(), overlay.of(0))
    }

    @Test fun displayTextGuessesItsKindForDrawing() {
        listOf("1,234.50", "$12", "12%", "1.2E+03", "2024-01-05", "-3", "12:30").forEach { assertEquals(it, SheetValueKind.NUMBER, displayKind(it)) }
        listOf("12 apples", "Total", "E5").forEach { assertEquals(it, SheetValueKind.TEXT, displayKind(it)) }
        assertEquals(SheetValueKind.BOOL, displayKind("TRUE"))
        assertEquals(SheetValueKind.ERROR, displayKind("#DIV/0!"))
        assertEquals(SheetValueKind.ERROR, displayKind("#N/A"))
        assertEquals(SheetValueKind.EMPTY, displayKind(""))
    }

    @Test fun newSheetsGetTheFirstFreeNumber() {
        assertEquals("Sheet1", newSheetName(emptyList()))
        assertEquals("Sheet3", newSheetName(listOf("Sheet1", "sheet2", "Data")))
    }

    @Test fun sheetNamesFollowExcelsRules() {
        assertNull(sheetNameProblem("Data", listOf("Sheet1", "data"), self = 1))
        assertNotNull(sheetNameProblem("Data", listOf("Sheet1", "data"), self = 0))
        assertNotNull(sheetNameProblem("", listOf("Sheet1"), self = 0))
        assertNotNull(sheetNameProblem("a/b", listOf("Sheet1"), self = 0))
        assertNotNull(sheetNameProblem("'quoted'", listOf("Sheet1"), self = 0))
        assertNotNull(sheetNameProblem("x".repeat(32), listOf("Sheet1"), self = 0))
    }

    @Test fun revealScrollsOnlyAsFarAsNeeded() {
        assertEquals(50.0, revealScroll(100.0, 50.0, 80.0, 200.0), 0.0)
        assertEquals(150.0, revealScroll(0.0, 300.0, 350.0, 200.0), 0.0)
        assertEquals(0.0, revealScroll(0.0, 10.0, 50.0, 200.0), 0.0)
        // Wider than the view: its start shows.
        assertEquals(300.0, revealScroll(0.0, 300.0, 600.0, 200.0), 0.0)
    }
}
