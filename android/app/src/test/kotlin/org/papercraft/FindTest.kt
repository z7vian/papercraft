package org.papercraft

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FindTest {
    private fun find(lines: List<String>, query: String, limit: Int = MAX_FIND_HITS) = runBlocking { findInLines(lines, query, limit) }

    @Test fun findsEveryMatchAcrossLinesInReadingOrder() {
        val lines = listOf("Apple pie", "", "no match here", "pineapple and APPLE")
        assertEquals(
            listOf(TextHit(0, 0, 5), TextHit(3, 4, 9), TextHit(3, 14, 19)),
            find(lines, "apple"),
        )
    }

    @Test fun overlappingMatchesAreAllFound() {
        assertEquals(listOf(TextHit(0, 0, 2), TextHit(0, 1, 3), TextHit(0, 2, 4)), find(listOf("aaaa"), "aa"))
        assertEquals(listOf(TextHit(0, 0, 3), TextHit(0, 2, 5)), find(listOf("ababa"), "aba"))
    }

    @Test fun emptyQueryFindsNothing() {
        assertTrue(find(listOf("anything", ""), "").isEmpty())
        assertTrue(find(emptyList(), "a").isEmpty())
    }

    @Test fun matchesStayWithinOneLine() {
        assertTrue(find(listOf("ends with ab", "cd starts"), "abcd").isEmpty())
    }

    @Test fun caseInsensitiveButAccentSensitive() {
        assertEquals(listOf(TextHit(0, 0, 5)), find(listOf("ÉCOLE"), "école"))
        assertTrue(find(listOf("école"), "ecole").isEmpty())
        assertTrue(find(listOf("ecole"), "école").isEmpty())
    }

    @Test fun stopsAtTheLimit() {
        assertEquals(listOf(TextHit(0, 0, 1), TextHit(0, 1, 2)), find(listOf("aaaa", "aaaa"), "a", limit = 2))
    }

    @Test fun nextAndPreviousWrapAround() {
        assertEquals(0, nextHit(-1, 3))
        assertEquals(1, nextHit(0, 3))
        assertEquals(0, nextHit(2, 3))
        assertEquals(2, previousHit(-1, 3))
        assertEquals(2, previousHit(0, 3))
        assertEquals(1, previousHit(2, 3))
        assertEquals(0, nextHit(0, 1))
        assertEquals(0, previousHit(0, 1))
        assertEquals(-1, nextHit(-1, 0))
        assertEquals(-1, previousHit(-1, 0))
    }

    @Test fun firstHitIsTheFirstFromWhereTheReaderIs() {
        val pages = listOf(PageHit(0, emptyList()), PageHit(2, emptyList()), PageHit(2, emptyList()), PageHit(5, emptyList()))
        assertEquals(0, pages.firstAtOrAfter(0) { it.page })
        assertEquals(1, pages.firstAtOrAfter(1) { it.page })
        assertEquals(1, pages.firstAtOrAfter(2) { it.page })
        assertEquals(3, pages.firstAtOrAfter(3) { it.page })
        // Past the last hit, it wraps to the first.
        assertEquals(0, pages.firstAtOrAfter(6) { it.page })
        assertEquals(-1, emptyList<PageHit>().firstAtOrAfter(0) { it.page })
    }

    @Test fun pdfPointBoxesScaleWithTheDrawnPage() {
        // A US Letter page (612 x 792 pt) drawn 1224 x 1584 px: every point is 2 px.
        val box = PageBox.fromPoints(61.2f, 79.2f, 122.4f, 99f, 612f, 792f)
        assertBox(PageBox(0.1f, 0.1f, 0.2f, 0.125f), box)
        assertBox(PageBox(122.4f, 158.4f, 244.8f, 198f), box.scaled(1224f, 1584f))
        // The same box on the page drawn at half the width and height.
        assertBox(PageBox(30.6f, 39.6f, 61.2f, 49.5f), box.scaled(306f, 396f))
    }

    @Test fun labelReadsNOfM() {
        assertNull(findLabel("", searching = false, failed = false, current = -1, count = 0, capped = false, pagesOnly = false))
        assertEquals("Searching…", findLabel("a", searching = true, failed = false, current = -1, count = 0, capped = false, pagesOnly = false))
        assertEquals("No matches", findLabel("a", searching = false, failed = false, current = -1, count = 0, capped = false, pagesOnly = false))
        assertEquals("2 of 7", findLabel("a", searching = false, failed = false, current = 1, count = 7, capped = false, pagesOnly = false))
        assertEquals("1 of 3 pages", findLabel("a", searching = false, failed = false, current = 0, count = 3, capped = false, pagesOnly = true))
        assertEquals("1 of 5000+", findLabel("a", searching = false, failed = false, current = 0, count = 5000, capped = true, pagesOnly = false))
    }

    private fun assertBox(expected: PageBox, actual: PageBox) {
        val d = 1e-3f
        assertEquals(expected.left, actual.left, d)
        assertEquals(expected.top, actual.top, d)
        assertEquals(expected.right, actual.right, d)
        assertEquals(expected.bottom, actual.bottom, d)
    }
}
