package org.papercraft

import android.content.Context
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.activity.compose.LocalActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.LifecycleOwner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * A document open in an editor. Every call blocks (it runs in the Rust engine): the shell makes them on the document's own
 * thread, and editors do too, through [EditScope.change] and [EditScope.read]. Never call them on the main thread.
 */
interface EditableDoc : AutoCloseable {
    /** True when the file was an old binary format converted on open: the first save asks for a new file. */
    val converted: Boolean

    /** Takes back the last change; false when there was none. */
    fun undo(): Boolean

    /** Makes the last undone change again; false when there was none. */
    fun redo(): Boolean

    fun canUndo(): Boolean

    fun canRedo(): Boolean

    /** The whole edited file, in the format [saveTargetOf] names for it. Editing can go on afterwards. */
    fun save(): ByteArray
}

/** An editor screen: opens [file] (its unsaved draft instead when [restore]) in [EditShell]; [onClose] leaves it. */
typealias EditorScreen = @Composable (file: DocFile, restore: Boolean, onClose: () -> Unit) -> Unit

/** Every editor and the office family it edits. Word and Excel plug in with one line each here. */
enum class Editor(val family: Category, val screen: EditorScreen) {
    Slides(Category.Slides, { file, restore, onClose -> SlideEditScreen(file, restore, onClose) }),
    Words(Category.Docs, { file, restore, onClose -> WordEditScreen(file, restore, onClose) }),
}

/** The editor for files with extension [ext], or null when Papercraft can't edit them: only files the office engine opens are editable. */
fun editorOf(ext: String): Editor? =
    if (ext in Viewer.Office.exts) Editor.entries.firstOrNull { ext in it.family.exts } else null

/** What closing an editor needs. */
enum class CloseAction { Close, AskToSave }

/**
 * Edit bookkeeping for one open document, free of Android types so unit tests drive it. A revision counts every change,
 * undo and redo; the state knows which revision the file and the draft hold, when the next draft is due (each edit restarts
 * the wait) and whether closing must ask. Thread-safe: changes land on the document's thread, the screen reads it.
 * [restored] starts from a draft: the edit isn't in the file yet, and the draft already holds it.
 */
class EditState(private val draftDelayMs: Long = DRAFT_DELAY_MS, restored: Boolean = false) {
    private var current = if (restored) 1 else 0
    private var savedRevision = 0
    private var draftedRevision = current
    private var lastEditAt = 0L

    val revision: Int @Synchronized get() = current

    /** True while the file lacks some change. */
    val dirty: Boolean @Synchronized get() = current != savedRevision

    /** Counts one change made at [now] (ms) and returns its revision. */
    @Synchronized
    fun edited(now: Long): Int {
        current++
        lastEditAt = now
        return current
    }

    /** True while neither the file nor the draft holds revision [rev]. */
    @Synchronized
    fun needsDraft(rev: Int): Boolean = rev > savedRevision && rev > draftedRevision

    /** How long after [now] to write the draft: [draftDelayMs] after the last edit, or null when nothing needs writing. */
    @Synchronized
    fun draftDelay(now: Long): Long? =
        if (needsDraft(current)) (lastEditAt + draftDelayMs - now).coerceAtLeast(0) else null

    /** The draft now holds [rev]. */
    @Synchronized
    fun drafted(rev: Int) {
        draftedRevision = maxOf(draftedRevision, rev)
    }

    /** The file now holds [rev]. Saving drops the draft, so later edits need a new one. */
    @Synchronized
    fun saved(rev: Int) {
        savedRevision = maxOf(savedRevision, rev)
        draftedRevision = savedRevision
    }

    /** The user threw the changes away: nothing needs saving or a draft any more. */
    @Synchronized
    fun discarded() {
        savedRevision = current
        draftedRevision = current
    }

    @Synchronized
    fun closeAction(): CloseAction = if (dirty) CloseAction.AskToSave else CloseAction.Close

    companion object {
        const val DRAFT_DELAY_MS = 3_000L
    }
}

/** What the editor does after a save failed. */
sealed interface SaveReaction {
    /** Show [text]; [offerSaveAs] puts "Save as" next to it. */
    data class Tell(val text: String, val offerSaveAs: Boolean) : SaveReaction

    /** Open the "Save as" picker right away. */
    data object PickNewFile : SaveReaction

    /** The file changed somewhere else: ask Overwrite / Save as copy / Cancel. */
    data object ConfirmOverwrite : SaveReaction
}

/** The reaction to [error], thrown by [SaveService] or by [EditableDoc.save]. The draft keeps the edit in every case. */
fun saveReaction(error: Throwable): SaveReaction = when (error) {
    is SaveException.NeedsSaveAs -> SaveReaction.PickNewFile
    is SaveException.ChangedOnDisk -> SaveReaction.ConfirmOverwrite
    is SaveException.ReadOnly -> SaveReaction.Tell("This file is read-only here. Save a copy to keep your changes.", true)
    is SaveException.Missing -> SaveReaction.Tell("The file was moved or deleted. Save a copy to keep your changes.", true)
    is SaveException.Unsupported -> SaveReaction.Tell("Papercraft can't save .${error.ext} files.", false)
    is SaveException.Incomplete ->
        SaveReaction.Tell("The file didn't save completely. Your changes are kept: try again or save a copy.", true)
    is SaveException.WriteFailed ->
        SaveReaction.Tell("The file couldn't be saved. Your changes are kept: try again or save a copy.", true)
    is OutOfMemoryError -> SaveReaction.Tell("This file is too big to save for now. Your changes are kept.", false)
    else -> SaveReaction.Tell(error.message ?: "The file couldn't be saved. Your changes are kept.", false)
}

/**
 * What an editor gets from [EditShell]: the open [doc] and ordered calls into it. Calls run on the document's own thread
 * one at a time, in the order they were made, so a read sees every change made before it.
 */
class EditScope<D : EditableDoc> internal constructor(
    /** The open document. Its calls block: make them inside [change] or [read]. */
    val doc: D,
    internal val state: EditState,
    internal val engine: CoroutineDispatcher,
    private val scope: CoroutineScope,
    private val tell: (String) -> Unit,
) {
    /** Bumps after every change, undo and redo: key re-reads and re-renders on it. */
    var version by mutableIntStateOf(0)
        private set

    /** Bumps after a save, which changes [dirty] without a change to the document. */
    internal var saves by mutableIntStateOf(0)

    /** True while the file lacks some change. Composables reading it recompose after edits and saves. */
    val dirty: Boolean get() = version >= 0 && saves >= 0 && state.dirty

    /**
     * Set by the editor: sends input it holds back (typing waiting for a pause) through [change].
     * The shell calls it before undo, redo, save, close and when the app goes to the background.
     */
    var commitPending: () -> Unit = {}

    /** Runs [block] as one change, then [then] with its result on the main thread. A failure shows its message and changes nothing. */
    fun <T> change(block: D.() -> T, then: (T) -> Unit = {}) {
        // Undispatched: the call is queued on the document's thread before this returns, so calls keep their order.
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            val result = try {
                withContext(engine) { doc.block().also { state.edited(System.currentTimeMillis()) } }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Throwable) {
                tell(e.message ?: "That change didn't work.")
                return@launch
            }
            version++
            then(result)
        }
    }

    /** Runs [block] after every call made before it, off the main thread. */
    suspend fun <T> read(block: D.() -> T): T = withContext(engine) { doc.block() }

    /** Shows [text] briefly at the bottom of the editor. */
    fun message(text: String) = tell(text)

    internal fun history(undo: Boolean) {
        commitPending()
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            val changed = try {
                withContext(engine) {
                    (if (undo) doc.undo() else doc.redo()).also { if (it) state.edited(System.currentTimeMillis()) }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Throwable) {
                tell(e.message ?: "That didn't work.")
                false
            }
            if (changed) version++
        }
    }
}

/** The shell's state and actions; the composable below draws it. */
private class EditShellState<D : EditableDoc>(
    val file: DocFile,
    val context: Context,
    val scope: CoroutineScope,
    targetState: MutableState<DocFile?>,
    fromDraftState: MutableState<Boolean>,
    savedAsNewState: MutableState<Boolean>,
    val onClose: () -> Unit,
) {
    /** Where saves go: null until opened, then the file, after "Save as" the new file. */
    var target by targetState

    /** True when the open edit lives in a draft, so a recreated screen reopens the draft rather than the file. */
    var fromDraft by fromDraftState

    /** True after "Save as": a converted file then saves in place. */
    var savedAsNew by savedAsNewState

    val saver = SaveService(context)
    val snackbar = SnackbarHostState()

    /** The document's own thread. */
    val engine = Dispatchers.IO.limitedParallelism(1)

    /** Serializes draft writes and saves, so a late draft never lands after the save that replaced it. */
    val files = Mutex()

    var edit by mutableStateOf<EditScope<D>?>(null)
    var busy by mutableStateOf(false)
    var askClose by mutableStateOf(false)
    var askOverwrite by mutableStateOf(false)
    var closeAfterSave = false
    var pickNewFile: (SaveAsRequest) -> Unit = {}

    /** Reads the file (or its draft) and opens it. Blocking. */
    fun load(open: (ByteArray) -> D): Pair<DocFile, D> {
        // A fresh version stamp: the viewer's may be from long ago, and the bytes are read now.
        val t = target ?: if (fromDraft) file else context.describe(file.uri)
        val bytes = if (fromDraft) saver.drafts.read(t) ?: error("The unsaved changes to this file are gone.") else context.readBytes(t)
        return t to open(bytes)
    }

    fun opened(t: DocFile, doc: D): EditScope<D> {
        target = t
        return EditScope(doc, EditState(restored = fromDraft), engine, scope, ::message).also { edit = it }
    }

    fun release(doc: D) {
        // After whatever is still running on the document's thread.
        CoroutineScope(engine).launch { runCatching { doc.close() } }
    }

    fun message(text: String, offerSaveAs: Boolean = false) {
        scope.launch {
            val result = snackbar.showSnackbar(
                text,
                actionLabel = if (offerSaveAs) "Save as" else null,
                duration = if (offerSaveAs) SnackbarDuration.Long else SnackbarDuration.Short,
            )
            if (result == SnackbarResult.ActionPerformed) saveAs()
        }
    }

    /** Writes the draft now if the file and the draft both lack the latest change. */
    suspend fun writeDraft(e: EditScope<D>) {
        val t = target ?: return
        try {
            val (rev, bytes) = withContext(engine) {
                val rev = e.state.revision
                if (e.state.needsDraft(rev)) rev to e.doc.save() else null
            } ?: return
            val wrote = withContext(NonCancellable + Dispatchers.IO) {
                files.withLock {
                    e.state.needsDraft(rev).also {
                        if (it) {
                            saver.drafts.put(t, bytes)
                            e.state.drafted(rev)
                        }
                    }
                }
            }
            if (wrote) fromDraft = true
        } catch (x: CancellationException) {
            throw x
        } catch (x: Throwable) {
            // Limit: a draft that can't be written (app storage full) costs crash safety silently and is retried at the next edit; lift it with a one-time notice.
        }
    }

    fun save(overwrite: Boolean = false) {
        val e = edit ?: return
        val t = target ?: return
        if (busy) return
        e.commitPending()
        if (e.doc.converted && !savedAsNew) return saveAs()
        busy = true
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            try {
                val (rev, bytes) = withContext(engine) { e.state.revision to e.doc.save() }
                val saved = withContext(NonCancellable + Dispatchers.IO) {
                    files.withLock { saver.save(t, bytes, overwrite).also { e.state.saved(rev) } }
                }
                saved(e, saved)
            } catch (x: CancellationException) {
                throw x
            } catch (x: Throwable) {
                react(saveReaction(x))
            } finally {
                busy = false
            }
        }
    }

    /** Opens the "Save as" picker; [savedTo] gets its answer. */
    fun saveAs() {
        val e = edit ?: return
        val t = target ?: return
        e.commitPending()
        val request = t.saveAsRequest() ?: return message("Papercraft can't save .${t.ext} files.")
        pickNewFile(request)
    }

    fun savedTo(uri: Uri?) {
        val e = edit
        if (uri == null || e == null) {
            closeAfterSave = false
            return
        }
        val from = target
        busy = true
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            try {
                val (rev, bytes) = withContext(engine) { e.state.revision to e.doc.save() }
                val saved = withContext(NonCancellable + Dispatchers.IO) {
                    files.withLock { saver.saveAs(from, uri, bytes).also { e.state.saved(rev) } }
                }
                savedAsNew = true
                saved(e, saved)
            } catch (x: CancellationException) {
                throw x
            } catch (x: Throwable) {
                react(saveReaction(x))
            } finally {
                busy = false
            }
        }
    }

    private fun saved(e: EditScope<D>, file: DocFile) {
        target = file
        fromDraft = false
        e.saves++
        if (closeAfterSave) onClose() else message("Saved")
    }

    private fun react(reaction: SaveReaction) {
        when (reaction) {
            SaveReaction.PickNewFile -> saveAs()
            SaveReaction.ConfirmOverwrite -> askOverwrite = true
            is SaveReaction.Tell -> {
                closeAfterSave = false
                message(reaction.text, reaction.offerSaveAs)
            }
        }
    }

    fun requestClose() {
        val e = edit ?: return onClose()
        e.commitPending()
        scope.launch(start = CoroutineStart.UNDISPATCHED) {
            withContext(engine) {} // after the input just sent
            when (e.state.closeAction()) {
                CloseAction.Close -> onClose()
                CloseAction.AskToSave -> askClose = true
            }
        }
    }

    fun saveAndClose() {
        askClose = false
        closeAfterSave = true
        save()
    }

    fun discardAndClose() {
        askClose = false
        val e = edit
        val t = target
        scope.launch {
            withContext(NonCancellable + Dispatchers.IO) {
                files.withLock {
                    e?.state?.discarded()
                    if (t != null) runCatching { saver.drafts.discard(t.uri) }
                }
            }
            onClose()
        }
    }

    fun overwrite() {
        askOverwrite = false
        save(overwrite = true)
    }

    fun saveCopy() {
        askOverwrite = false
        saveAs()
    }

    fun cancelOverwrite() {
        askOverwrite = false
        closeAfterSave = false
    }
}

/**
 * The frame every editor sits in: top bar (close, undo, redo, Save, "Save as" in the overflow menu), saving with every
 * failure explained, the unsaved-changes question on back, and a crash-safe draft written [EditState.DRAFT_DELAY_MS]
 * after the last edit. [open] turns the file's bytes into the document (blocking, off the main thread); [content] is
 * the editor itself. [restore] opens the file's draft (from [Drafts.pending]) instead of the file.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun <D : EditableDoc> EditShell(
    file: DocFile,
    restore: Boolean,
    onClose: () -> Unit,
    open: (ByteArray) -> D,
    content: @Composable (EditScope<D>) -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val targetState = rememberSaveable(stateSaver = DocFileSaver) { mutableStateOf<DocFile?>(null) }
    val fromDraftState = rememberSaveable { mutableStateOf(restore) }
    val savedAsNewState = rememberSaveable { mutableStateOf(false) }
    val shell = remember {
        EditShellState<D>(file, context.applicationContext, scope, targetState, fromDraftState, savedAsNewState, onClose)
    }
    val picker = rememberLauncherForActivityResult(CreateDocumentAs()) { shell.savedTo(it) }
    SideEffect { shell.pickNewFile = { picker.launch(it) } }

    val opened by produceState<Result<EditScope<D>>?>(null) {
        var doc: D? = null
        try {
            val loaded = withContext(NonCancellable + Dispatchers.IO) {
                runCatching { shell.load(open) }.onSuccess { doc = it.second }
            }
            value = loaded.map { (t, d) -> shell.opened(t, d) }
            awaitCancellation()
        } finally {
            doc?.let(shell::release)
        }
    }
    val edit = shell.edit
    BackHandler { shell.requestClose() }

    if (edit != null) {
        // The draft: each edit restarts the wait.
        LaunchedEffect(edit, edit.version, edit.saves) {
            val wait = edit.state.draftDelay(System.currentTimeMillis()) ?: return@LaunchedEffect
            delay(wait)
            shell.writeDraft(edit)
        }
        // Going to the background may be the last chance before the process is killed.
        val lifecycle = (LocalActivity.current as? LifecycleOwner)?.lifecycle
        DisposableEffect(lifecycle, edit) {
            val observer = LifecycleEventObserver { _, event ->
                if (event == Lifecycle.Event.ON_STOP) {
                    edit.commitPending()
                    scope.launch { shell.writeDraft(edit) }
                }
            }
            lifecycle?.addObserver(observer)
            onDispose { lifecycle?.removeObserver(observer) }
        }
    }

    Scaffold(
        topBar = { EditTopBar(shell, edit) },
        snackbarHost = { SnackbarHost(shell.snackbar) },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding).imePadding()) {
            val result = opened
            when {
                result == null -> Box(Modifier.fillMaxSize(), Alignment.Center) {
                    CircularProgressIndicator(Modifier.semantics { contentDescription = "Loading" })
                }
                result.isFailure -> Text(
                    friendlyError(result.exceptionOrNull()!!),
                    Modifier.fillMaxSize().padding(32.dp).semantics { liveRegion = LiveRegionMode.Polite },
                    textAlign = TextAlign.Center,
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> content(result.getOrThrow())
            }
        }
    }

    if (shell.askClose) {
        AlertDialog(
            onDismissRequest = { shell.askClose = false },
            title = { Text("Save changes?") },
            text = { Text("Your changes to \"${(shell.target ?: file).name}\" aren't saved yet.") },
            confirmButton = { TextButton(shell::saveAndClose) { Text("Save") } },
            dismissButton = {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton({ shell.askClose = false }) { Text("Cancel") }
                    TextButton(shell::discardAndClose) { Text("Discard") }
                }
            },
        )
    }
    if (shell.askOverwrite) {
        AlertDialog(
            onDismissRequest = shell::cancelOverwrite,
            title = { Text("File changed elsewhere") },
            text = {
                Text("\"${(shell.target ?: file).name}\" was changed somewhere else since you opened it. Replace that version with yours, or save yours as a copy?")
            },
            confirmButton = { TextButton(shell::overwrite) { Text("Overwrite") } },
            dismissButton = {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton(shell::cancelOverwrite) { Text("Cancel") }
                    TextButton(shell::saveCopy) { Text("Save as copy") }
                }
            },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun <D : EditableDoc> EditTopBar(shell: EditShellState<D>, edit: EditScope<D>?) {
    val shown = shell.target ?: shell.file
    TopAppBar(
        title = {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                FileBadge(shown.ext, size = 28.dp)
                Column {
                    Text(shown.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val status = when {
                        shell.busy -> "Saving…"
                        edit?.dirty == true -> "Unsaved changes"
                        else -> null
                    }
                    status?.let { Text(it, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1) }
                }
            }
        },
        navigationIcon = { IconButton(shell::requestClose) { Icon(painterResource(R.drawable.ic_close), "Close") } },
        actions = {
            if (edit != null) {
                val history by produceState(false to false, edit, edit.version) {
                    value = try {
                        edit.read { canUndo() to canRedo() }
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Throwable) {
                        false to false
                    }
                }
                IconButton({ edit.history(undo = true) }, enabled = history.first) { Icon(painterResource(R.drawable.ic_undo), "Undo") }
                IconButton({ edit.history(undo = false) }, enabled = history.second) { Icon(painterResource(R.drawable.ic_redo), "Redo") }
                TextButton({ shell.save() }, enabled = !shell.busy) { Text("Save") }
                var menu by remember { mutableStateOf(false) }
                Box {
                    IconButton({ menu = true }) { Icon(painterResource(R.drawable.ic_more), "More options") }
                    DropdownMenu(menu, { menu = false }) {
                        DropdownMenuItem(text = { Text("Save as") }, enabled = !shell.busy, onClick = { menu = false; shell.saveAs() })
                    }
                }
            }
        },
        colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
    )
}
