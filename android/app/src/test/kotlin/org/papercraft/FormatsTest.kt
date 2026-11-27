package org.papercraft

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FormatsTest {
    @Test fun newFormatsHaveTheirOwnTypeLabel() {
        assertEquals("RTF document", typeLabel("rtf"))
        assertEquals("XML file", typeLabel("xml"))
        assertEquals("Print file", typeLabel("prn"))
        assertEquals("dBASE table", typeLabel("dbf"))
        assertEquals("DIF file", typeLabel("dif"))
    }

    @Test fun otherTypesFallBackToCategoryOrExtension() {
        assertEquals("Document", typeLabel("docx"))
        assertEquals("Spreadsheet", typeLabel("xlsx"))
        assertEquals("Presentation", typeLabel("pptx"))
        assertEquals("Picture", typeLabel("png"))
        assertEquals("ZIP file", typeLabel("zip"))
        assertEquals("File", typeLabel(""))
    }

    @Test fun homeHintNamesTheNewFormats() {
        for (noun in listOf("RTF document", "dBASE table", "DIF file", "print file")) assertTrue(noun, noun in openableHint)
        assertTrue(openableHint.startsWith("a document, "))
        assertTrue(openableHint.endsWith(" or saved web page"))
    }
}
