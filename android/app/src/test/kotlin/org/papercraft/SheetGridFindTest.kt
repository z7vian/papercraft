package org.papercraft

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SheetGridFindTest {
    // Sheet 1 is hidden, so it isn't among the tabs searched.
    private val tabs = listOf(0, 2, 3)
    private val cellsBySheet = mapOf(
        0 to listOf(5 to 1),
        1 to listOf(0 to 0),
        2 to listOf(0 to 0, 1 to 3),
        3 to listOf(2 to 2),
    )

    @Test fun matchesRunInTabOrderThenRowByRow() {
        val searched = ArrayList<Int>()
        val hits = runBlocking { findInSheets(tabs) { sheet, _ -> searched += sheet; cellsBySheet.getValue(sheet) } }
        assertEquals(listOf(CellHit(0, 5, 1), CellHit(2, 0, 0), CellHit(2, 1, 3), CellHit(3, 2, 2)), hits)
        assertEquals(tabs, searched)
    }

    @Test fun limitStopsTheSearchAcrossSheets() {
        val asked = ArrayList<Pair<Int, Int>>()
        val hits = runBlocking { findInSheets(tabs, limit = 2) { sheet, left -> asked += sheet to left; cellsBySheet.getValue(sheet) } }
        assertEquals(listOf(CellHit(0, 5, 1), CellHit(2, 0, 0)), hits)
        // Each sheet is asked only for the hits that still fit; none once the limit is reached.
        assertEquals(listOf(0 to 2, 2 to 1), asked)
    }

    @Test fun firstHitShownIsOnTheCurrentSheetOrAfterIt() {
        val hits = listOf(CellHit(0, 5, 1), CellHit(2, 0, 0), CellHit(2, 1, 3), CellHit(3, 2, 2))
        assertEquals(0, hits.firstFromSheet(0, tabs))
        assertEquals(1, hits.firstFromSheet(2, tabs))
        assertEquals(3, hits.firstFromSheet(3, tabs))
        // Nothing on or after the last sheet: wrap to the first hit.
        assertEquals(0, listOf(CellHit(0, 1, 1)).firstFromSheet(3, tabs))
        assertEquals(-1, emptyList<CellHit>().firstFromSheet(0, tabs))
    }

    @Test fun tableMatchesCellsRowByRowIgnoringCaseWithinShownColumns() {
        val rows = listOf(listOf("Apple", "x"), listOf("b", "pineapple", "APPLE"), listOf("apple"))
        val find = { query: String, limit: Int -> runBlocking { findInTable(rows, 2, query, limit) } }
        assertEquals(listOf(CellHit(0, 0, 0), CellHit(0, 1, 1), CellHit(0, 2, 0)), find("apple", MAX_FIND_HITS))
        assertEquals(listOf(CellHit(0, 0, 0)), find("APPLE", 1))
        assertTrue(find("", MAX_FIND_HITS).isEmpty())
    }

    @Test fun tableJumpLeavesTheRowJustUnderTheColumnLetters() {
        assertEquals(0, tableJumpItem(0))
        assertEquals(41, tableJumpItem(41))
    }

    @Test fun revealScrollsOnlyAsFarAsNeeded() {
        // Rows 20 px, two frozen; 100 px of the scrolling part show.
        val rows = GridAxis(20f, emptyList())
        assertEquals(0.0, rows.reveal(3, 3, 2, 0.0, 100.0), 0.0)
        assertEquals(80.0, rows.reveal(10, 10, 2, 0.0, 100.0), 0.0)
        assertEquals(0.0, rows.reveal(2, 2, 2, 80.0, 100.0), 0.0)
        // Frozen rows always show.
        assertEquals(80.0, rows.reveal(1, 1, 2, 80.0, 100.0), 0.0)
        // Taller than the screen: its top comes first.
        assertEquals(160.0, rows.reveal(10, 20, 2, 0.0, 100.0), 0.0)
        assertEquals(200.0, revealSpan(500.0, 600.0, 0.0, 400.0), 0.0)
        assertEquals(300.0, revealSpan(500.0, 600.0, 300.0, 400.0), 0.0)
    }

    @Test fun matchBoxesSitInTheirPaneOnScreen() {
        // Columns 64 px, rows 20 px, one frozen column and two frozen rows, scale 1, headers 40 x 30.
        fun viewport(scrollX: Double = 0.0, scrollY: Double = 0.0) =
            GridViewport(GridAxis(64f, emptyList()), GridAxis(20f, emptyList()), 1, 2, 1f, scrollX, scrollY, 1000f, 2000f, 40f, 30f)
        assertEquals(GridBox(104f, 70f, 168f, 90f), viewport().boxOf(GridRect(2, 1, 2, 1)))
        assertEquals(GridBox(40f, 30f, 104f, 50f), viewport().boxOf(GridRect(0, 0, 0, 0)))
        assertEquals(GridBox(104f, 70f, 232f, 110f), viewport().boxOf(GridRect(2, 1, 3, 2)))
        // Scrolled half a column: cut at the frozen column's edge, never drawn over it.
        assertEquals(GridBox(104f, 70f, 136f, 90f), viewport(scrollX = 32.0).boxOf(GridRect(2, 1, 2, 1)))
        assertNull(viewport().boxOf(GridRect(500, 1, 500, 1)))
        assertNull(viewport(scrollY = 40.0).boxOf(GridRect(2, 1, 3, 1)))
    }

    @Test fun matchInAMergeFillsTheMerge() {
        val merges = GridMerges(listOf(GridRect(1, 1, 2, 3)))
        assertEquals(GridRect(1, 1, 2, 3), merges.span(1, 1))
        assertEquals(GridRect(0, 0, 0, 0), merges.span(0, 0))
    }
}
