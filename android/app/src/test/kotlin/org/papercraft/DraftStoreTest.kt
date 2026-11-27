package org.papercraft

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class DraftStoreTest {
    @get:Rule val tmp = TemporaryFolder()

    private fun store() = DraftStore(File(tmp.root, "drafts"))

    @Test fun roundTripsBinaryBytes() {
        val s = store()
        val bytes = ByteArray(70_000) { (it * 31).toByte() } // includes '\n' bytes and every byte value
        s.put("a1", "content://docs/1\tReport.docx", bytes)
        assertArrayEquals(bytes, s.read("a1"))
        // A fresh store on the same folder (the next launch) sees the same copy.
        assertArrayEquals(bytes, store().read("a1"))
    }

    @Test fun putReplacesThePreviousCopy() {
        val s = store()
        s.put("a1", "h1", byteArrayOf(1, 2, 3))
        s.put("a1", "h2", byteArrayOf(9))
        assertArrayEquals(byteArrayOf(9), s.read("a1"))
        val all = s.list()
        assertEquals(1, all.size)
        assertEquals("h2", all[0].header)
        assertEquals(1L, all[0].size)
        // No temp files left after a normal put.
        assertEquals(listOf("a1.draft"), File(tmp.root, "drafts").list()!!.toList())
    }

    @Test fun listsNewestFirstForRecovery() {
        val s = store()
        s.put("old", "first", byteArrayOf(1))
        s.put("new", "second", byteArrayOf(2, 2))
        File(tmp.root, "drafts/old.draft").setLastModified(1_000_000)
        File(tmp.root, "drafts/new.draft").setLastModified(2_000_000)
        val all = store().list()
        assertEquals(listOf("new", "old"), all.map { it.key })
        assertEquals(listOf("second", "first"), all.map { it.header })
        assertEquals(listOf(2L, 1L), all.map { it.size })
        assertEquals(2_000_000L, all[0].savedAt)
    }

    @Test fun deleteForgetsTheCopy() {
        val s = store()
        s.put("a1", "h", byteArrayOf(1))
        s.delete("a1")
        assertNull(s.read("a1"))
        assertTrue(s.list().isEmpty())
        s.delete("never-saved".filter { it.isLetterOrDigit() }) // deleting nothing is fine
    }

    @Test fun crashLeftoversAreIgnoredAndCleaned() {
        val s = store()
        s.put("good", "h", byteArrayOf(7))
        val dir = File(tmp.root, "drafts")
        // A put that died before its rename: a stale temp file.
        val stale = File(dir, "good123.tmp").apply { writeBytes(byteArrayOf(1, 2)); setLastModified(1_000) }
        // A draft cut off inside its header can't say whose it is.
        File(dir, "cut.draft").writeBytes("content://no-newline".toByteArray())
        val all = s.list()
        assertEquals(listOf("good"), all.map { it.key })
        assertFalse(stale.exists())
        assertNull(s.read("cut"))
        assertArrayEquals(byteArrayOf(7), s.read("good"))
    }

    @Test fun emptyOrMissingFolderListsNothing() {
        assertTrue(store().list().isEmpty())
        assertNull(store().read("a1"))
    }

    @Test(expected = IllegalArgumentException::class)
    fun headerMustBeOneLine() {
        store().put("a1", "two\nlines", byteArrayOf())
    }

    @Test(expected = IllegalArgumentException::class)
    fun keyCannotEscapeTheFolder() {
        store().read("../secrets")
    }

    @Test fun draftKeyIsStableHexPerUri() {
        val k = draftKey("content://com.android.externalstorage.documents/document/primary%3AReport.docx")
        assertEquals(64, k.length)
        assertTrue(k.all { it in '0'..'9' || it in 'a'..'f' })
        assertEquals(k, draftKey("content://com.android.externalstorage.documents/document/primary%3AReport.docx"))
        assertNotEquals(k, draftKey("content://com.android.externalstorage.documents/document/primary%3AReport2.docx"))
    }
}
