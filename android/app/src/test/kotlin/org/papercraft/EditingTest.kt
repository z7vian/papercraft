package org.papercraft

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException

class EditingTest {
    private fun state(restored: Boolean = false) = EditState(draftDelayMs = 3_000, restored = restored)

    // Draft debounce and autosave.

    @Test fun nothingToDraftBeforeAnEdit() {
        assertNull(state().draftDelay(now = 10_000))
    }

    @Test fun draftIsDueAfterThePause() {
        val s = state()
        s.edited(now = 1_000)
        assertEquals(3_000L, s.draftDelay(now = 1_000))
        assertEquals(1_000L, s.draftDelay(now = 3_000))
        assertEquals(0L, s.draftDelay(now = 9_000))
    }

    @Test fun eachEditRestartsTheWait() {
        val s = state()
        s.edited(now = 1_000)
        s.edited(now = 2_500)
        assertEquals(3_000L, s.draftDelay(now = 2_500))
        s.edited(now = 5_000)
        assertEquals(2_000L, s.draftDelay(now = 6_000))
    }

    @Test fun writtenDraftStopsTheTimerUntilTheNextEdit() {
        val s = state()
        val rev = s.edited(now = 0)
        assertTrue(s.needsDraft(rev))
        s.drafted(rev)
        assertNull(s.draftDelay(now = 10_000))
        s.edited(now = 20_000)
        assertEquals(3_000L, s.draftDelay(now = 20_000))
    }

    @Test fun draftOfAnOlderRevisionKeepsTheTimer() {
        val s = state()
        val first = s.edited(now = 0)
        s.edited(now = 1_000) // made while the first draft was being written
        s.drafted(first)
        assertEquals(3_000L, s.draftDelay(now = 1_000))
    }

    @Test fun lateDraftAfterSaveIsSkipped() {
        val s = state()
        val rev = s.edited(now = 0)
        s.saved(s.revision)
        assertFalse(s.needsDraft(rev))
        assertNull(s.draftDelay(now = 5_000))
    }

    @Test fun saveDropsTheDraftSoNewerEditsNeedAnother() {
        val s = state()
        val saving = s.edited(now = 0)
        val newer = s.edited(now = 100)
        s.drafted(newer) // written before the save, which then removes the draft file
        s.saved(saving)
        assertTrue(s.needsDraft(newer))
        assertEquals(0L, s.draftDelay(now = 10_000))
    }

    @Test fun discardStopsEveryDraft() {
        val s = state()
        val rev = s.edited(now = 0)
        s.discarded()
        assertFalse(s.needsDraft(rev))
        assertNull(s.draftDelay(now = 5_000))
        assertFalse(s.dirty)
    }

    @Test fun restoredDraftNeedsNoNewDraft() {
        val s = state(restored = true)
        assertNull(s.draftDelay(now = 5_000))
        assertTrue(s.dirty)
    }

    // Unsaved-changes guard.

    @Test fun cleanEditorClosesRightAway() {
        assertEquals(CloseAction.Close, state().closeAction())
    }

    @Test fun editAsksBeforeClosing() {
        val s = state()
        s.edited(now = 0)
        assertEquals(CloseAction.AskToSave, s.closeAction())
    }

    @Test fun savedEditClosesAndTheNextEditAsksAgain() {
        val s = state()
        s.saved(s.edited(now = 0))
        assertEquals(CloseAction.Close, s.closeAction())
        s.edited(now = 1)
        assertEquals(CloseAction.AskToSave, s.closeAction())
    }

    @Test fun editsDuringASaveStillAsk() {
        val s = state()
        val saving = s.edited(now = 0)
        s.edited(now = 1)
        s.saved(saving)
        assertEquals(CloseAction.AskToSave, s.closeAction())
    }

    @Test fun olderSaveFinishingLastDoesNotUnsave() {
        val s = state()
        val first = s.edited(now = 0)
        val second = s.edited(now = 1)
        s.saved(second)
        s.saved(first)
        assertEquals(CloseAction.Close, s.closeAction())
    }

    @Test fun restoredDraftAsksBeforeClosing() {
        assertEquals(CloseAction.AskToSave, state(restored = true).closeAction())
    }

    // Save errors.

    @Test fun needsSaveAsOpensThePicker() {
        val target = saveTargetOf("ppt")!!
        assertEquals(SaveReaction.PickNewFile, saveReaction(SaveException.NeedsSaveAs(target)))
    }

    @Test fun outsideChangeAsksToOverwrite() {
        assertEquals(SaveReaction.ConfirmOverwrite, saveReaction(SaveException.ChangedOnDisk(FileStamp(1, 1), FileStamp(2, 2))))
    }

    @Test fun readOnlyAndMissingOfferSaveAs() {
        for (e in listOf(SaveException.ReadOnly(), SaveException.Missing())) {
            val r = saveReaction(e) as SaveReaction.Tell
            assertTrue(r.offerSaveAs)
            assertTrue(r.text, r.text.contains("Save a copy"))
        }
    }

    @Test fun failedWritesSayTheChangesAreKept() {
        for (e in listOf(SaveException.Incomplete(100, 40), SaveException.WriteFailed(IOException("disk")))) {
            val r = saveReaction(e) as SaveReaction.Tell
            assertTrue(r.offerSaveAs)
            assertTrue(r.text, r.text.contains("changes are kept"))
        }
    }

    @Test fun unsupportedNamesTheFormat() {
        assertEquals(SaveReaction.Tell("Papercraft can't save .txt files.", false), saveReaction(SaveException.Unsupported("txt")))
    }

    @Test fun engineErrorShowsItsMessage() {
        assertEquals(
            SaveReaction.Tell("Can't save this presentation: bad part", false),
            saveReaction(IllegalStateException("Can't save this presentation: bad part")),
        )
        assertEquals(SaveReaction.Tell("This file is too big to save for now. Your changes are kept.", false), saveReaction(OutOfMemoryError()))
    }

    @Test fun everySaveExceptionHasAClearMessage() {
        val all = listOf(
            SaveException.Unsupported("txt"), SaveException.NeedsSaveAs(saveTargetOf("doc")!!), SaveException.ReadOnly(),
            SaveException.ChangedOnDisk(FileStamp(1, 1), FileStamp(2, 2)), SaveException.Missing(),
            SaveException.Incomplete(10, 5), SaveException.WriteFailed(IOException()),
        )
        for (e in all) {
            when (val r = saveReaction(e)) {
                is SaveReaction.Tell -> assertTrue(e.javaClass.simpleName, r.text.isNotBlank() && !r.text.contains("Exception"))
                SaveReaction.PickNewFile, SaveReaction.ConfirmOverwrite -> Unit
            }
        }
    }
}
