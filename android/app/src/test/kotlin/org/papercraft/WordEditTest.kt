package org.papercraft

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.papercraft.core.DocAlign
import org.papercraft.core.DocBlock
import org.papercraft.core.DocParagraph
import org.papercraft.core.DocRun
import org.papercraft.core.DocRunProperty
import org.papercraft.core.DocTableCell
import org.papercraft.core.DocTableRow

class WordEditTest {
    private fun run(text: String, bold: Boolean = false, sizePt: Float? = null, editable: Boolean = true) = DocRun(
        text = text, bold = bold, italic = false, underline = false, strike = false,
        sizePt = sizePt, color = null, font = null, editable = editable,
    )

    private fun para(index: Int, vararg runs: DocRun, styleId: String? = null, styleName: String? = null, listLevel: Int? = null) =
        DocParagraph(
            index = index.toUInt(), text = runs.joinToString("") { it.text }, styleId = styleId, styleName = styleName,
            alignment = DocAlign.LEFT, listLevel = listLevel?.toUInt(), runs = runs.toList(),
        )

    private fun wordPara(text: String, joinsPrevious: Boolean = true, joinsNext: Boolean = true) =
        wordModel(listOf(DocBlock.Paragraph(para(0, run(text))))).paragraphs.getValue(0)
            .copy(joinsPrevious = joinsPrevious, joinsNext = joinsNext)

    // ---- Field text -> engine offsets ----

    @Test fun typingAtTheEndAppends() {
        assertEquals(FieldChange(5, 5, "!"), fieldChange("Hello", "Hello!", 6))
    }

    @Test fun theCaretSettlesWhereARepeatedLetterWentIn() {
        // "aa" -> "aaa": the same text whether the new "a" went first, second or last.
        assertEquals(FieldChange(0, 0, "a"), fieldChange("aa", "aaa", 1))
        assertEquals(FieldChange(1, 1, "a"), fieldChange("aa", "aaa", 2))
        assertEquals(FieldChange(2, 2, "a"), fieldChange("aa", "aaa", 3))
    }

    @Test fun deletingAndReplacing() {
        assertEquals(FieldChange(1, 2, ""), fieldChange("abc", "ac", 1))
        assertEquals(FieldChange(0, 1, ""), fieldChange("aab", "ab", 0))
        assertEquals(FieldChange(4, 9, "there"), fieldChange("Hey world!", "Hey there!", 9))
        assertNull(fieldChange("same", "same", 2))
    }

    @Test fun offsetsCountUtf16UnitsAndNeverCutASurrogatePair() {
        // An emoji is two UTF-16 units: text after it sits two offsets on.
        assertEquals(FieldChange(1, 1, "\uD83D\uDE00"), fieldChange("ab", "a\uD83D\uDE00b", 3))
        // 😀 -> 😁 share the high surrogate; the change still covers the whole character.
        assertEquals(FieldChange(1, 3, "\uD83D\uDE01"), fieldChange("a\uD83D\uDE00b", "a\uD83D\uDE01b", 3))
        // Characters that share only the low surrogate (caret at the start, so the shared tail would count as unchanged).
        assertEquals(FieldChange(0, 2, "\uD83E\uDE00"), fieldChange("\uD83D\uDE00", "\uD83E\uDE00", 0))
    }

    @Test fun typingJoinsIntoOneChangeWhileItStaysInsideIt() {
        val typed = FieldChange(3, 3, "he").then(FieldChange(5, 5, "y"))
        assertEquals(FieldChange(3, 3, "hey"), typed)
        assertEquals(FieldChange(3, 3, "hy"), typed!!.then(FieldChange(4, 5, "")))
        assertEquals(FieldChange(3, 5, "X"), FieldChange(3, 5, "XY").then(FieldChange(4, 5, "")))
        // Backspace past the typed text, or typing elsewhere, starts a new change.
        assertNull(FieldChange(3, 3, "he").then(FieldChange(2, 3, "")))
        assertNull(FieldChange(3, 3, "he").then(FieldChange(9, 9, "x")))
    }

    @Test fun lockedTextCanBeRemovedWholeButNotCut() {
        val link = listOf(WordLock(2, 5))
        assertTrue(FieldChange(3, 3, "x").cuts(link))
        assertTrue(FieldChange(0, 4, "").cuts(link))
        assertFalse(FieldChange(2, 2, "x").cuts(link))
        assertFalse(FieldChange(5, 5, "x").cuts(link))
        assertFalse(FieldChange(1, 6, "").cuts(link))
    }

    @Test fun locksMoveWithTheText() {
        val locks = listOf(WordLock(0, 2), WordLock(5, 8))
        assertEquals(listOf(WordLock(0, 2), WordLock(7, 10)), locks.after(FieldChange(3, 3, "ab")))
        assertEquals(listOf(WordLock(0, 2)), locks.after(FieldChange(4, 9, "")))
    }

    @Test fun formattingNeverStartsOrEndsInsideLockedText() {
        val locks = listOf(WordLock(2, 5), WordLock(8, 10))
        assertEquals(2 to 10, outsideLocks(3, 9, locks))
        assertEquals(0 to 5, outsideLocks(0, 5, locks))
    }

    // ---- Enter, Backspace and Delete ----

    @Test fun enterSplitsWhereTheNewlineWasTyped() {
        val (typed, cuts) = splitAtNewlines(FieldChange(3, 3, "\n"))!!
        assertEquals(FieldChange(3, 3, ""), typed)
        assertEquals(listOf(3), cuts)
    }

    @Test fun pastedLinesBecomeParagraphs() {
        // "a\nb\nc" pasted over [1, 2): the paragraph reads x + "abc" + rest, split after "a" and after "b".
        val (typed, cuts) = splitAtNewlines(FieldChange(1, 2, "a\nb\nc"))!!
        assertEquals(FieldChange(1, 2, "abc"), typed)
        assertEquals(listOf(2, 3), cuts)
        assertNull(splitAtNewlines(FieldChange(0, 0, "abc")))
    }

    @Test fun enterAfterAnExistingLineBreakSplitsAfterIt() {
        // The field shows line breaks as "\n" too; the caret tells the old one from the new one.
        assertEquals(listOf(1), splitAtNewlines(fieldChange("x\ny", "x\n\ny", 2)!!)!!.second)
        assertEquals(listOf(2), splitAtNewlines(fieldChange("x\ny", "x\n\ny", 3)!!)!!.second)
    }

    @Test fun backspaceAtTheStartJoinsWithThePreviousParagraph() {
        val p = wordPara("Hello")
        assertEquals(WordKeyAction.JoinPrevious, keyAction(WordKey.Backspace, false, 0, 0, p))
        assertEquals(WordKeyAction.None, keyAction(WordKey.Backspace, false, 1, 1, p))
        assertEquals(WordKeyAction.None, keyAction(WordKey.Backspace, false, 0, 3, p))
        assertEquals(WordKeyAction.None, keyAction(WordKey.Backspace, false, 0, 0, p.copy(joinsPrevious = false)))
    }

    @Test fun deleteAtTheEndJoinsWithTheNextParagraph() {
        val p = wordPara("Hello")
        assertEquals(WordKeyAction.JoinNext, keyAction(WordKey.Delete, false, 5, 5, p))
        assertEquals(WordKeyAction.None, keyAction(WordKey.Delete, false, 4, 4, p))
        assertEquals(WordKeyAction.None, keyAction(WordKey.Delete, false, 5, 5, p.copy(joinsNext = false)))
    }

    @Test fun shiftEnterIsALineBreakAndEnterIsLeftToTheField() {
        val p = wordPara("Hello")
        assertEquals(WordKeyAction.LineBreak, keyAction(WordKey.Enter, true, 2, 2, p))
        assertEquals(WordKeyAction.None, keyAction(WordKey.Enter, false, 2, 2, p))
    }

    // ---- Selection style ----

    @Test fun selectionStyleIsWhatEveryRunShares() {
        // "Hello " plain 11 pt, "world" bold 11 pt, "!" bold 14 pt.
        val spans = wordModel(
            listOf(DocBlock.Paragraph(para(0, run("Hello ", sizePt = 11f), run("world", bold = true, sizePt = 11f), run("!", bold = true, sizePt = 14f)))),
        ).paragraphs.getValue(0).spans
        assertEquals(true, selectionLook(spans, 6, 11).bold)
        assertEquals(11f, selectionLook(spans, 6, 11).sizePt)
        assertNull(selectionLook(spans, 0, 11).bold)
        assertEquals(11f, selectionLook(spans, 0, 11).sizePt)
        assertEquals(true, selectionLook(spans, 8, 12).bold)
        assertNull(selectionLook(spans, 8, 12).sizePt)
        assertEquals(false, selectionLook(spans, 0, 6).bold)
        assertEquals(WordLook(null, null, null, null, null, null, null), selectionLook(emptyList(), 0, 0))
    }

    // ---- Block list -> display model ----

    @Test fun blocksBecomeTheDisplayModel() {
        val heading = para(0, run("Intro "), run("link", editable = false), styleId = "Heading1", styleName = "heading 1")
        val body = para(1, run("Body"))
        val cellA = listOf(para(2, run("A1")), para(3, run("A2")))
        val cellB = listOf(para(4, run("B")))
        val after = para(5, run("Item"), listLevel = 1)
        val model = wordModel(
            listOf(
                DocBlock.Paragraph(heading),
                DocBlock.Paragraph(body),
                DocBlock.Table(listOf(DocTableRow(listOf(DocTableCell(cellA), DocTableCell(cellB))))),
                DocBlock.Paragraph(after),
            ),
        )
        assertEquals(4, model.blocks.size)
        assertEquals((0..5).toSet(), model.paragraphs.keys)

        val h = model.paragraphs.getValue(0)
        assertEquals(1, h.heading)
        assertEquals(listOf(0 to 6, 6 to 10), h.spans.map { it.start to it.end })
        assertEquals(listOf(WordLock(6, 10)), h.locks)
        assertFalse(h.joinsPrevious)
        assertTrue(h.joinsNext)

        // A paragraph before a table can't be joined with it; cell paragraphs join only inside their cell.
        assertTrue(model.paragraphs.getValue(1).joinsPrevious)
        assertFalse(model.paragraphs.getValue(1).joinsNext)
        assertFalse(model.paragraphs.getValue(2).joinsPrevious)
        assertTrue(model.paragraphs.getValue(2).joinsNext)
        assertTrue(model.paragraphs.getValue(3).joinsPrevious)
        assertFalse(model.paragraphs.getValue(3).joinsNext)
        assertFalse(model.paragraphs.getValue(4).joinsPrevious)
        assertFalse(model.paragraphs.getValue(5).joinsPrevious)

        val table = model.blocks[2] as WordBlock.Table
        assertEquals(listOf(listOf(listOf(2, 3), listOf(4))), table.rows.map { row -> row.map { cell -> cell.map { it.index } } })
        assertEquals(2, model.blockOf(4))
        assertEquals(3, model.blockOf(5))
        assertEquals(1, model.paragraphs.getValue(5).listLevel)
        assertNull(model.paragraphs.getValue(5).heading)
    }

    @Test fun headingLevelsComeFromTheStyle() {
        assertEquals(2, headingLevel("Heading2", "heading 2"))
        assertEquals(0, headingLevel("Title", "Title"))
        assertEquals(3, headingLevel("Heading3", null))
        // A localized style id with the built-in English name.
        assertEquals(1, headingLevel("berschrift1", "heading 1"))
        assertNull(headingLevel("Normal", "Normal"))
        assertNull(headingLevel(null, null))
    }

    @Test fun documentFontsMapToGenericFamilies() {
        assertEquals(WordFont.Serif, wordFont("Times New Roman"))
        assertEquals(WordFont.Sans, wordFont("Microsoft Sans Serif"))
        assertEquals(WordFont.Mono, wordFont("Courier New"))
        assertEquals(WordFont.Sans, wordFont("Calibri"))
        assertEquals(WordFont.Sans, wordFont(null))
    }

    @Test fun clearFormattingClearsEveryPropertyAndSetsNone() {
        val f = clearFormat()
        assertEquals(DocRunProperty.entries.toSet(), f.clear.toSet())
        assertEquals(listOf(null, null, null, null, null, null, null), listOf(f.bold, f.italic, f.underline, f.strike, f.sizePt, f.color, f.font))
        assertTrue(runFormat(bold = true).clear.isEmpty())
    }

    @Test fun coloursParseAsOpaqueArgb() {
        assertEquals(0xFFC00000L, argbOf("C00000"))
        assertEquals(0xFF0070C0L, argbOf("#0070C0"))
        assertNull(argbOf("auto"))
        assertNull(argbOf(null))
    }
}
