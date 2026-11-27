package org.papercraft

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SavePolicyTest {
    private val docx = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    private val xlsx = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    private val pptx = "application/vnd.openxmlformats-officedocument.presentationml.presentation"

    @Test fun legacyFormatsSaveAsOoxml() {
        val expected = mapOf(
            "doc" to "docx", "dot" to "docx", "wps" to "docx", "wpt" to "docx",
            "xls" to "xlsx", "xlt" to "xlsx", "xla" to "xlsx", "et" to "xlsx", "ett" to "xlsx",
            "ppt" to "pptx", "pot" to "pptx", "pps" to "pptx", "dps" to "pptx", "dpt" to "pptx",
        )
        val mimes = mapOf("docx" to docx, "xlsx" to xlsx, "pptx" to pptx)
        for ((legacy, ooxml) in expected) {
            assertEquals(legacy, SaveTarget(ooxml, mimes.getValue(ooxml), inPlace = false), saveTargetOf(legacy))
        }
    }

    @Test fun ooxmlSavesInPlaceWithItsOwnType() {
        assertEquals(SaveTarget("docx", docx, inPlace = true), saveTargetOf("docx"))
        assertEquals(SaveTarget("xlsx", xlsx, inPlace = true), saveTargetOf("xlsx"))
        assertEquals(SaveTarget("pptx", pptx, inPlace = true), saveTargetOf("pptx"))
        assertEquals(SaveTarget("xlsm", "application/vnd.ms-excel.sheet.macroEnabled.12", inPlace = true), saveTargetOf("xlsm"))
        assertEquals(SaveTarget("ppsx", "application/vnd.openxmlformats-officedocument.presentationml.slideshow", inPlace = true), saveTargetOf("ppsx"))
        assertEquals(SaveTarget("pdf", "application/pdf", inPlace = true), saveTargetOf("pdf"))
    }

    @Test fun extensionCaseDoesNotMatter() {
        assertEquals(saveTargetOf("docx"), saveTargetOf("DOCX"))
        assertEquals(saveTargetOf("doc"), saveTargetOf("Doc"))
    }

    @Test fun formatsWeDontWriteHaveNoTarget() {
        for (ext in listOf("txt", "csv", "png", "html", "mhtml", "tif", "")) assertNull(ext, saveTargetOf(ext))
    }

    @Test fun suggestedNameSwapsTheExtension() {
        fun name(name: String, ext: String) = suggestedName(name, ext, saveTargetOf(ext)!!)
        assertEquals("Report.docx", name("Report.doc", "doc"))
        assertEquals("Q3 budget.xlsx", name("Q3 budget.XLS", "xls"))
        assertEquals("deck.v2.pptx", name("deck.v2.dps", "dps"))
        assertEquals("Letter.docx", name("Letter.docx", "docx"))
        // Extension came from the MIME type, not the name.
        assertEquals("scan.docx", name("scan", "doc"))
        assertEquals("Untitled.xlsx", name(".et", "et"))
    }

    @Test fun sameStampIsUnchanged() {
        assertFalse(changedOnDisk(FileStamp(100, 5000), FileStamp(100, 5000)))
    }

    @Test fun sizeOrTimeChangeIsDetected() {
        assertTrue(changedOnDisk(FileStamp(100, 5000), FileStamp(101, 5000)))
        assertTrue(changedOnDisk(FileStamp(100, 5000), FileStamp(100, 6000)))
        assertTrue(changedOnDisk(FileStamp(100, 5000), FileStamp(0, 5000)))
    }

    @Test fun unknownValuesCantProveAChange() {
        assertFalse(changedOnDisk(FileStamp(-1, -1), FileStamp(100, 5000)))
        assertFalse(changedOnDisk(FileStamp(100, -1), FileStamp(100, 5000)))
        assertFalse(changedOnDisk(FileStamp(100, 0), FileStamp(100, 5000)))
        assertFalse(changedOnDisk(FileStamp(100, 5000), FileStamp(-1, -1)))
        // Size still counts when the time is unknown.
        assertTrue(changedOnDisk(FileStamp(100, -1), FileStamp(200, -1)))
    }
}
