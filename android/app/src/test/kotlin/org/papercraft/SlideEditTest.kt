package org.papercraft

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.papercraft.core.DeckParagraph
import org.papercraft.core.DeckRect
import org.papercraft.core.DeckRun
import org.papercraft.core.DeckShape
import org.papercraft.core.DeckShapeKind
import org.papercraft.core.DeckTextStyle

class SlideEditTest {
    private fun style(bold: Boolean? = null, sizePt: Double? = null, color: String? = null) =
        DeckTextStyle(bold = bold, italic = null, underline = null, strike = null, sizePt = sizePt, color = color, font = null)

    private fun para(vararg runs: Pair<String, DeckTextStyle>) = DeckParagraph(runs = runs.map { DeckRun(text = it.first, style = it.second) })

    private fun shape(id: String, x: Long, y: Long, w: Long, h: Long, kind: DeckShapeKind = DeckShapeKind.SHAPE, group: String? = null) =
        DeckShape(
            id = id, kind = kind, name = id, placeholder = null, bounds = DeckRect(x = x, y = y, width = w, height = h),
            rotationDeg = 0.0, groupId = group, paragraphs = listOf(para("text" to style())),
        )

    @Test fun textJoinsParagraphsWithNewlines() {
        val s = shape("1", 0, 0, 1, 1).copy(paragraphs = listOf(para("Hello " to style(), "world" to style(bold = true)), para("Next" to style())))
        assertEquals("Hello world\nNext", shapeText(s))
    }

    @Test fun softBreaksRoundTripAtTheSameOffsets() {
        val engine = "a\u000Bb\nc"
        val field = fieldText(engine)
        assertEquals(engine.length, field.length)
        assertEquals("a\u2028b\nc", field)
        assertEquals(engine, engineText(field))
    }

    @Test fun noSelectionFormatsTheWholeText() {
        assertEquals(0 to 11, formatRange(4, 4, 11))
        assertEquals(2 to 7, formatRange(7, 2, 11))
        assertEquals(3 to 11, formatRange(3, 40, 11))
    }

    @Test fun rangeStyleIsWhatEveryRunShares() {
        // "Hello world" + "\n" + "Big": offsets 0-5 plain, 6-10 bold, 12-14 bold 24 pt.
        val paras = listOf(
            para("Hello " to style(sizePt = 18.0), "world" to style(bold = true, sizePt = 18.0)),
            para("Big" to style(bold = true, sizePt = 24.0)),
        )
        assertEquals(true, rangeStyle(paras, 6, 11).bold)
        assertEquals(18.0, rangeStyle(paras, 6, 11).sizePt)
        assertNull(rangeStyle(paras, 0, 11).bold)
        assertEquals(18.0, rangeStyle(paras, 0, 11).sizePt)
        assertEquals(true, rangeStyle(paras, 6, 15).bold)
        assertNull(rangeStyle(paras, 6, 15).sizePt)
        assertEquals(24.0, rangeStyle(paras, 12, 15).sizePt)
    }

    @Test fun emptyTextHasNoStyle() {
        assertEquals(style(), rangeStyle(emptyList(), 0, 0))
    }

    @Test fun tapPicksTheTopmostTextShape() {
        val back = shape("back", 0, 0, 100, 100)
        val front = shape("front", 50, 50, 100, 100, kind = DeckShapeKind.TEXT_BOX)
        val picture = shape("pic", 0, 0, 200, 200, kind = DeckShapeKind.PICTURE)
        val shapes = listOf(back, front, picture)
        assertEquals("front", shapeAt(shapes, 60, 60)?.id)
        assertEquals("back", shapeAt(shapes, 10, 10)?.id)
        assertNull(shapeAt(shapes, 190, 190))
    }

    @Test fun groupMembersAreNotPicked() {
        val member = shape("m", 0, 0, 100, 100, group = "g")
        assertNull(shapeAt(listOf(member), 10, 10))
    }
}
