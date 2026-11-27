package org.papercraft

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SheetGridMathTest {
    // Rows 0-1 default 20 px, rows 2-3 40 px, row 5 hidden.
    private val axis = GridAxis(20f, listOf(GridRun(5, 5, 0f), GridRun(2, 3, 40f)))

    @Test fun trackStartsAddUpSizeRuns() {
        assertEquals(listOf(0.0, 20.0, 40.0, 80.0, 120.0, 140.0, 140.0, 160.0), (0..7).map { axis.start(it) })
        assertEquals(40f, axis.size(3))
        assertEquals(0f, axis.size(5))
        assertEquals(20f, axis.size(6))
    }

    @Test fun offsetsFindTheirTrackAndSkipHiddenOnes() {
        assertEquals(0, axis.indexAt(0.0))
        assertEquals(0, axis.indexAt(19.9))
        assertEquals(1, axis.indexAt(20.0))
        assertEquals(2, axis.indexAt(40.0))
        assertEquals(2, axis.indexAt(79.0))
        assertEquals(4, axis.indexAt(139.0))
        // Row 5 is hidden, so the offset where it would be belongs to row 6.
        assertEquals(6, axis.indexAt(140.0))
        assertEquals(7, axis.indexAt(160.0))
        assertEquals(0, axis.indexAt(-5.0))
    }

    @Test fun millionRowSheetsNeedNoPerRowWork() {
        val rows = GridAxis(20f, emptyList())
        assertEquals(1_000_000, rows.indexAt(20.0 * 1_000_000))
        assertEquals(20_971_500.0, rows.start(1_048_575), 0.0)
    }

    @Test fun visibleWindowCoversPartlyShownTracksOnly() {
        assertEquals(0..3, axis.visible(0, 0.0, 100.0))
        // A track starting exactly at the far edge isn't shown.
        assertEquals(0..1, axis.visible(0, 0.0, 40.0))
        // Scrolling counts from the end of the frozen tracks.
        assertEquals(2..3, axis.visible(1, 30.0, 60.0))
    }

    @Test fun viewportSplitsHeadersFrozenPanesAndBody() {
        val v = viewport(scrollX = 0.0)
        assertEquals(1, v.frozenCols)
        assertEquals(2, v.frozenRows)
        assertEquals(168f, v.bodyX)
        assertEquals(110f, v.bodyY)
        assertEquals(1..7, v.bodyCols)
        assertEquals(2..9, viewport(scrollX = 100.0).bodyCols)

        assertNull(v.colAt(30f))
        assertEquals(0, v.colAt(100f))
        assertEquals(1, v.colAt(168f))
        assertEquals(2, v.colAt(168f + 128f))
        assertNull(v.rowAt(20f))
        assertEquals(0, v.rowAt(50f))
        assertEquals(3, v.rowAt(110f + 41f))

        assertEquals(160.0, v.maxScrollX(10), 1e-9)
        assertEquals(1015.0, v.maxScrollY(100, bottomPad = 0f), 1e-9)
        assertEquals(1040.0, v.maxScrollY(100, bottomPad = 50f), 1e-9)
    }

    @Test fun freezeTooBigForTheScreenIsDropped() {
        val v = GridViewport(GridAxis(64f, emptyList()), GridAxis(20f, emptyList()), 10, 2, 2f, 0.0, 0.0, 1000f, 2000f, 40f, 30f)
        assertEquals(0, v.frozenCols)
        assertEquals(2, v.frozenRows)
    }

    private fun viewport(scrollX: Double) =
        GridViewport(GridAxis(64f, emptyList()), GridAxis(20f, emptyList()), 1, 2, 2f, scrollX, 0.0, 1000f, 2000f, 40f, 30f)

    @Test fun blocksCoverEveryRangePair() {
        assertEquals(setOf(GridBlockKey(0, 0)), gridBlocks(listOf(0..63), listOf(0..15)))
        assertEquals(
            setOf(GridBlockKey(0, 0), GridBlockKey(0, 1), GridBlockKey(1, 0), GridBlockKey(1, 1)),
            gridBlocks(listOf(60..70), listOf(10..20)),
        )
        assertEquals(emptySet<GridBlockKey>(), gridBlocks(listOf(0 until 0), listOf(0..15)))
    }

    @Test fun mergeLookupFindsCoveringRange() {
        val title = GridRect(0, 0, 0, 3)
        val block = GridRect(5, 1, 9, 2)
        val tall = GridRect(100, 0, 101, 0)
        val merges = GridMerges(listOf(tall, title, block))
        assertEquals(title, merges.at(0, 2))
        assertEquals(block, merges.at(7, 2))
        assertEquals(tall, merges.at(101, 0))
        assertNull(merges.at(7, 3))
        assertNull(merges.at(4, 1))
        assertNull(merges.at(10, 1))

        fun window(rows: IntRange, cols: IntRange) = ArrayList<GridRect>().also { merges.inWindow(rows, cols, it) }
        assertEquals(listOf(block), window(6..20, 0..1))
        assertEquals(emptyList<GridRect>(), window(0..0, 4..10))
        assertEquals(listOf(tall), window(90..200, 0..5))
    }

    @Test fun columnLettersMatchExcel() {
        val names = mapOf(0 to "A", 25 to "Z", 26 to "AA", 51 to "AZ", 52 to "BA", 701 to "ZZ", 702 to "AAA", 16383 to "XFD")
        for ((index, name) in names) assertEquals(name, sheetColumnName(index))
    }

    @Test fun webAndMailLinksOpenAsTheyAre() {
        assertEquals(GridLinkTarget.Web("https://example.com/a?b=1"), gridLinkTarget("https://example.com/a?b=1", null))
        assertEquals(GridLinkTarget.Web("MAILTO:me@example.org"), gridLinkTarget("MAILTO:me@example.org", null))
        assertEquals(GridLinkTarget.Web("https://www.example.com"), gridLinkTarget("www.example.com", null))
    }

    @Test fun workbookLocationsJumpToTheirFirstCell() {
        assertEquals(GridLinkTarget.Place("Sheet2", 2, 1), gridLinkTarget(null, "Sheet2!B3"))
        assertEquals(GridLinkTarget.Place("Q3 'final'", 9, 2), gridLinkTarget("#'Q3 ''final'''!\$C\$10", null))
        assertEquals(GridLinkTarget.Place(null, 4, 0), gridLinkTarget(null, "A5"))
        assertEquals(GridLinkTarget.Place("Data", 1, 1), gridLinkTarget(null, "Data!b2:d9"))
        // A file link with a location still jumps.
        assertEquals(GridLinkTarget.Place("Sheet1", 0, 0), gridLinkTarget("report.xlsx", "Sheet1!A1"))
    }

    @Test fun linksWithNowhereToGoAreIgnored() {
        assertNull(gridLinkTarget(null, "MyName"))
        assertNull(gridLinkTarget("report.xlsx", null))
        assertNull(gridLinkTarget("javascript:alert(1)", null))
        assertNull(gridLinkTarget("file:///sdcard/a.xlsx", null))
        assertNull(gridLinkTarget(null, "Sheet1!XFE1"))
        assertNull(gridLinkTarget(null, "Sheet1!A0"))
        assertNull(gridLinkTarget(null, "[Book2.xlsx]Sheet1!A1"))
        assertNull(gridLinkTarget(null, "'Unclosed!A1"))
        assertNull(gridLinkTarget(null, null))
    }

    @Test fun sheetNamesMatchRegardlessOfCase() {
        assertEquals(1, gridSheetIndex(listOf("Sheet1", "Data"), "data"))
        assertNull(gridSheetIndex(listOf("Sheet1", "Data"), "Missing"))
    }

    @Test fun textSpillsOnlyIntoFreeNeighbours() {
        val cols = GridAxis(10f, emptyList())
        assertEquals(2..4, cols.spill(2, 25.0, GridSpill.RIGHT, 0, 10) { true })
        assertEquals(2..3, cols.spill(2, 25.0, GridSpill.RIGHT, 0, 10) { it != 4 })
        assertEquals(3..5, cols.spill(5, 25.0, GridSpill.LEFT, 0, 10) { true })
        assertEquals(4..6, cols.spill(5, 30.0, GridSpill.BOTH, 0, 10) { true })
        assertEquals(5..5, cols.spill(5, 8.0, GridSpill.BOTH, 0, 10) { true })
        assertEquals(0..1, cols.spill(1, 50.0, GridSpill.LEFT, 0, 10) { true })
    }

    @Test fun zoomKeepsThePointUnderTheFingers() {
        val scroll = gridZoomScroll(100.0, 50f, 50f, 1f, 2f)
        assertEquals(125.0, scroll, 1e-9)
        assertEquals(100.0 + 50 / 1.0, scroll + 50 / 2.0, 1e-9)
    }
}
