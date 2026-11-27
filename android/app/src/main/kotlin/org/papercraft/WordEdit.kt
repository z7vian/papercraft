package org.papercraft

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.max
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.papercraft.core.DocAlign
import org.papercraft.core.DocEditor
import org.papercraft.core.DocPosition
import org.papercraft.core.DocRange
import org.papercraft.core.DocRun
import org.papercraft.core.DocRunFormat
import org.papercraft.core.DocStyle
import org.papercraft.core.OfficeFamily

/** A Word document in the edit shell. */
class WordDoc(val editor: DocEditor) : EditableDoc {
    override val converted = editor.converted()
    override fun undo() = editor.undo()
    override fun redo() = editor.redo()
    override fun canUndo() = editor.canUndo()
    override fun canRedo() = editor.canRedo()
    override fun save() = editor.save()
    override fun close() = editor.close()
}

/** The Word editor: .docx and its kin, and old .doc/.dot/.wps/.wpt (converted, saved as .docx). */
@Composable
fun WordEditScreen(file: DocFile, restore: Boolean, onClose: () -> Unit) =
    EditShell(file, restore, onClose, open = { WordDoc(DocEditor.open(it, OfficeFamily.DOCUMENT)) }) { WordEditing(it) }

private const val TYPING_PAUSE_MS = 700L
/** Sizes in sp per point: a point is 4/3 of a CSS pixel, and an sp is about one CSS pixel on a phone. */
private const val SP_PER_PT = 4f / 3f
/** Width of a Letter page at the core's scale 1.0 (96 dpi), until the first drawing tells the real one. */
private const val PAGE_PX = 816f
private const val PAGE_RATIO = 8.5f / 11f
private const val PAGE_BREAK = '\u000C'
private const val LOCKED = "Links, fields and tracked changes can't be edited yet."
private val LIST_INDENT = 20.dp
private val MIN_CELL = 112.dp
private val SIZES = listOf(8f, 9f, 10f, 10.5f, 11f, 12f, 14f, 16f, 18f, 20f, 22f, 24f, 26f, 28f, 36f, 48f, 72f)

/** Word's standard colours, as RRGGBB with their names; "auto" is the automatic colour. */
private val COLORS = listOf(
    "auto" to "Automatic", "000000" to "Black", "FFFFFF" to "White", "C00000" to "Dark red", "FF0000" to "Red",
    "FFC000" to "Orange", "FFFF00" to "Yellow", "92D050" to "Light green", "00B050" to "Green", "00B0F0" to "Light blue",
    "0070C0" to "Blue", "002060" to "Dark blue", "7030A0" to "Purple",
)

private val ALIGNS = listOf(DocAlign.LEFT to "Left", DocAlign.CENTER to "Center", DocAlign.RIGHT to "Right", DocAlign.JUSTIFY to "Justify")

/** Documents are written for white paper, like the rendered pages: text and the document's own colours sit on white. */
private val PAPER = Color.White
private val INK = Color.Black
private val LOCK_TINT = Color.Black.copy(alpha = 0.08f)

/**
 * The editor's state: the document model and the paragraph being edited. Typing shows in the field at once and reaches
 * the engine after a pause as one change ([pending], in the engine's offsets); structural edits (split, join) wait for
 * the engine and lock the field meanwhile ([busy]).
 */
private class WordSession(val edit: EditScope<WordDoc>, val scope: CoroutineScope) {
    var model by mutableStateOf<WordModel?>(null)
    var styles by mutableStateOf<List<DocStyle>>(emptyList())

    /** Bumps to read the document again after a call that may fail (a failure doesn't bump [EditScope.version]). */
    var reloads by mutableIntStateOf(0)

    /** The paragraph being edited, or null. */
    var index by mutableStateOf<Int?>(null)
    var value by mutableStateOf(TextFieldValue())
    var locks by mutableStateOf<List<WordLock>>(emptyList())
    var busy by mutableStateOf(false)
    private var pending: FieldChange? = null

    /** Counts calls that change the text; a read started before the latest one is stale and dropped. */
    private var sent = 0

    val active: WordPara? get() = index?.let { model?.paragraphs?.get(it) }

    suspend fun load() {
        val seq = sent
        val m = edit.read { wordModel(editor.blocks()) }
        if (seq != sent) return
        model = m
        sync(m)
    }

    /** The field shows the engine's text after every read, unless it holds typing the engine hasn't got. */
    private fun sync(m: WordModel) {
        val i = index ?: return
        val p = m.paragraphs[i] ?: return close()
        if (pending != null) return
        locks = p.locks
        if (value.text != p.text) {
            val sel = value.selection
            value = TextFieldValue(p.text, TextRange(sel.start.coerceIn(0, p.text.length), sel.end.coerceIn(0, p.text.length)))
        }
    }

    private fun open(i: Int, text: String, cursor: Int, locks: List<WordLock> = emptyList()) {
        index = i
        value = TextFieldValue(text, TextRange(cursor.coerceIn(0, text.length)))
        pending = null
        this.locks = locks
    }

    private fun close() {
        index = null
        value = TextFieldValue()
        pending = null
        locks = emptyList()
    }

    /** Sends held-back typing to the engine. */
    fun commit() {
        val i = index ?: return
        val c = pending ?: return
        pending = null
        sent++
        edit.change({ editor.replaceText(range(i, c.start, c.end), c.text) })
        reloads++
    }

    /** Holds [change] back with the typing before it, or sends that typing first when the two don't join. */
    private fun accept(change: FieldChange) {
        val merged = pending?.then(change)
        if (pending != null && merged == null) commit()
        pending = (merged ?: change).takeUnless { it.start == it.end && it.text.isEmpty() }
        locks = locks.after(change)
    }

    /** Makes [change] in the field, caret after it. */
    private fun put(change: FieldChange) {
        accept(change)
        val text = value.text.replaceRange(change.start, change.end, change.text)
        value = TextFieldValue(text, TextRange(change.start + change.text.length))
    }

    fun type(new: TextFieldValue) {
        if (busy || index == null) return
        val change = fieldChange(value.text, new.text, new.selection.end) ?: run { value = new; return }
        if (change.cuts(locks)) return edit.message(LOCKED)
        val split = splitAtNewlines(change)
        if (split == null) {
            accept(change)
            value = new
        } else {
            enter(split.first, split.second)
        }
    }

    /** Enter: [typed] goes in, then the paragraph splits at [cuts]; the caret lands in the last part. */
    private fun enter(typed: FieldChange, cuts: List<Int>) {
        val i = index ?: return
        put(typed)
        val text = value.text
        val last = cuts.last()
        val cursor = typed.start + typed.text.length - last
        structural({ for (at in cuts.asReversed()) editor.splitParagraph(DocPosition(i.toUInt(), at.toUInt())) }) {
            open(i + cuts.size, text.substring(last), cursor)
        }
    }

    /** True when the field handled [key]. */
    fun key(key: WordKey, shift: Boolean): Boolean {
        val p = active ?: return false
        if (busy) return true
        val sel = value.selection
        when (keyAction(key, shift, sel.min, sel.max, p.copy(text = value.text))) {
            WordKeyAction.None -> return false
            WordKeyAction.LineBreak -> {
                val change = FieldChange(sel.min, sel.max, "\n")
                if (change.cuts(locks)) edit.message(LOCKED) else put(change)
            }
            WordKeyAction.JoinPrevious -> {
                val prev = model?.paragraphs?.get(p.index - 1) ?: return true
                val text = value.text
                structural({ editor.mergeParagraph((p.index - 1).toUInt()) }) { open(p.index - 1, prev.text + text, prev.text.length) }
            }
            WordKeyAction.JoinNext -> {
                val next = model?.paragraphs?.get(p.index + 1) ?: return true
                val text = value.text
                structural({ editor.mergeParagraph(p.index.toUInt()) }) { open(p.index, text + next.text, text.length) }
            }
        }
        return true
    }

    /** Runs [block], which renumbers paragraphs, then [then] when it worked; the field takes no input until it's done. */
    private fun structural(block: WordDoc.() -> Unit, then: () -> Unit) {
        commit()
        busy = true
        sent++
        edit.change(block) { then() }
        scope.launch {
            try {
                edit.read {} // after the change, whether or not it worked
            } finally {
                busy = false
                reloads++
            }
        }
    }

    fun activate(p: WordPara, offset: Int) {
        if (busy || index == p.index) return
        commit()
        open(p.index, p.text, offset, p.locks)
    }

    fun done() {
        commit()
        close()
    }

    /** The formatting of the selection, or of the whole paragraph when nothing is selected. */
    fun look(p: WordPara): WordLook {
        val sel = value.selection
        val (start, end) = formatRange(sel.min, sel.max, value.text.length)
        return selectionLook(p.spans, start, end)
    }

    fun format(format: DocRunFormat) {
        val i = index ?: return
        commit()
        val sel = value.selection
        val (start, end) = formatRange(sel.min, sel.max, value.text.length).let { (s, e) -> outsideLocks(s, e, locks) }
        if (start >= end) return
        edit.change({ editor.setRunFormat(range(i, start, end), format) })
    }

    fun align(alignment: DocAlign) {
        val i = index ?: return
        commit()
        edit.change({ editor.setAlignment(i.toUInt(), alignment) })
    }

    fun style(id: String) {
        val i = index ?: return
        commit()
        edit.change({ editor.setStyle(i.toUInt(), id) })
    }

    fun pageBreak() {
        if (busy || index == null) return
        val at = value.selection.max
        val change = FieldChange(at, at, PAGE_BREAK.toString())
        if (change.cuts(locks)) return edit.message(LOCKED)
        put(change)
        commit()
    }

    /** A document without paragraphs gets one to type in. */
    fun addFirst() {
        sent++
        edit.change({ editor.insertParagraph(0u, "") }) { open(0, "", 0) }
    }

    private fun range(i: Int, start: Int, end: Int) =
        DocRange(DocPosition(i.toUInt(), start.toUInt()), DocPosition(i.toUInt(), end.toUInt()))
}

@Composable
private fun WordEditing(edit: EditScope<WordDoc>) {
    val scope = rememberCoroutineScope()
    val s = remember { WordSession(edit, scope) }
    var pages by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(edit.version, s.reloads) {
        try {
            s.load()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            edit.message(e.message ?: "Papercraft can't read this document.")
        }
    }
    LaunchedEffect(Unit) {
        s.styles = try {
            edit.read { editor.styles() }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            emptyList()
        }
    }
    SideEffect { edit.commitPending = { s.commit() } }
    // Typing goes to the engine after a pause, as one undo step.
    LaunchedEffect(s.value.text) {
        delay(TYPING_PAUSE_MS)
        s.commit()
    }

    val model = s.model ?: return Box(Modifier.fillMaxSize(), Alignment.Center) {
        CircularProgressIndicator(Modifier.semantics { contentDescription = "Loading" })
    }
    Column(Modifier.fillMaxSize()) {
        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (pages) WordPages(edit) else WordBody(s, model)
        }
        if (!pages && s.index == null && model.blocks.isNotEmpty()) {
            Text(
                "Tap a paragraph to edit it.",
                Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        FormatBar(s, pages) { on ->
            if (on) s.done()
            pages = on
        }
    }
}

@Composable
private fun WordBody(s: WordSession, model: WordModel) {
    if (model.blocks.isEmpty()) {
        Column(Modifier.fillMaxSize().padding(32.dp), Arrangement.Center, Alignment.CenterHorizontally) {
            Text("This document has no text yet.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            TextButton(s::addFirst) { Text("Add text") }
        }
        return
    }
    val list = rememberLazyListState()
    // Keep the paragraph being edited on screen, e.g. after Enter at the bottom.
    LaunchedEffect(s.index) {
        val i = s.index ?: return@LaunchedEffect
        val block = s.model?.blockOf(i) ?: return@LaunchedEffect
        if (list.layoutInfo.visibleItemsInfo.none { it.index == block }) list.animateScrollToItem(block)
    }
    LazyColumn(
        Modifier.fillMaxSize().background(PAPER),
        state = list,
        contentPadding = PaddingValues(horizontal = 20.dp, vertical = 16.dp),
    ) {
        items(model.blocks.size, contentType = { if (model.blocks[it] is WordBlock.Table) 1 else 0 }) { i ->
            when (val block = model.blocks[i]) {
                is WordBlock.Para -> Paragraph(s, block.para)
                is WordBlock.Table -> Table(s, block)
            }
        }
    }
}

/**
 * Cells side by side, each at least [MIN_CELL] wide; wide tables scroll sideways.
 * Limit: cells don't span rows or columns and take one width each; the core reporting grid spans and column widths lifts it.
 */
@Composable
private fun Table(s: WordSession, table: WordBlock.Table) {
    val columns = table.rows.maxOfOrNull { it.size }?.coerceAtLeast(1) ?: 1
    val line = MaterialTheme.colorScheme.outline
    BoxWithConstraints(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        val width = max(maxWidth / columns, MIN_CELL)
        Column(Modifier.horizontalScroll(rememberScrollState()).border(1.dp, line)) {
            table.rows.forEach { row ->
                Row(Modifier.height(IntrinsicSize.Min)) {
                    row.forEach { cell ->
                        Column(Modifier.width(width).fillMaxHeight().border(0.5.dp, line).padding(6.dp)) {
                            cell.forEach { Paragraph(s, it) }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun Paragraph(s: WordSession, p: WordPara) {
    val style = paragraphStyle(p)
    val indent = p.listLevel?.let { LIST_INDENT * (it + 1) } ?: 0.dp
    Row(Modifier.fillMaxWidth().padding(start = indent, top = 3.dp, bottom = 3.dp)) {
        // Limit: numbered list items show a bullet too; the core reporting each item's number text lifts it.
        if (p.listLevel != null) Text("•", Modifier.padding(end = 6.dp), style = style)
        if (s.index == p.index) ParagraphField(s, p, style) else ParagraphText(s, p, style)
    }
}

@Composable
private fun RowScope.ParagraphText(s: WordSession, p: WordPara, style: TextStyle) {
    var layout by remember { mutableStateOf<TextLayoutResult?>(null) }
    Text(
        styled(shownText(p.text), p.spans, emptyList()),
        Modifier.weight(1f)
            .pointerInput(p) {
                detectTapGestures { at -> s.activate(p, layout?.getOffsetForPosition(at) ?: p.text.length) }
            }
            .semantics {
                onClick("Edit") { s.activate(p, p.text.length); true }
                if (p.heading != null) heading()
            },
        style = style,
        onTextLayout = { layout = it },
    )
}

@Composable
private fun RowScope.ParagraphField(s: WordSession, p: WordPara, style: TextStyle) {
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    // The engine's run styles line up with the field only while it shows the engine's text.
    val spans = if (s.value.text == p.text) p.spans else emptyList()
    val locks = s.locks
    val look = remember(spans, locks) {
        VisualTransformation { text -> TransformedText(styled(text.text, spans, locks), OffsetMapping.Identity) }
    }
    BasicTextField(
        s.value,
        s::type,
        Modifier.weight(1f).focusRequester(focus).onPreviewKeyEvent { e ->
            if (e.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
            val key = when (e.key) {
                Key.Enter, Key.NumPadEnter -> WordKey.Enter
                Key.Backspace -> WordKey.Backspace
                Key.Delete -> WordKey.Delete
                else -> return@onPreviewKeyEvent false
            }
            s.key(key, e.isShiftPressed)
        },
        readOnly = s.busy,
        textStyle = style,
        cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
        visualTransformation = look,
    )
    LaunchedEffect(p.index) {
        focus.requestFocus()
        keyboard?.show()
    }
}

/** What a paragraph's text shows as when not being edited: a page break starts a new line. */
private fun shownText(text: String) = text.replace(PAGE_BREAK, '\n')

/** [text] with each run's look and a tint on the [locks]. Ranges past the text are clipped. */
private fun styled(text: String, spans: List<WordSpan>, locks: List<WordLock>): AnnotatedString = buildAnnotatedString {
    append(text)
    for (span in spans) {
        val end = minOf(span.end, text.length)
        if (span.start < end) addStyle(spanStyle(span.run), span.start, end)
    }
    for (lock in locks) {
        val end = minOf(lock.end, text.length)
        if (lock.start < end) addStyle(SpanStyle(background = LOCK_TINT), lock.start, end)
    }
}

private fun spanStyle(run: DocRun) = SpanStyle(
    color = argbOf(run.color)?.let { Color(it) } ?: Color.Unspecified,
    fontSize = run.sizePt?.let { (it * SP_PER_PT).sp } ?: TextUnit.Unspecified,
    fontWeight = if (run.bold) FontWeight.Bold else FontWeight.Normal,
    fontStyle = if (run.italic) FontStyle.Italic else FontStyle.Normal,
    fontFamily = fontFamily(run.font),
    textDecoration = TextDecoration.combine(
        listOfNotNull(TextDecoration.Underline.takeIf { run.underline }, TextDecoration.LineThrough.takeIf { run.strike }),
    ),
)

private fun fontFamily(name: String?): FontFamily = when (wordFont(name)) {
    WordFont.Sans -> FontFamily.SansSerif
    WordFont.Serif -> FontFamily.Serif
    WordFont.Mono -> FontFamily.Monospace
}

/** The paragraph's base look: its first run's size and font (what an empty paragraph types in), its alignment. */
@Composable
private fun paragraphStyle(p: WordPara): TextStyle {
    val first = p.spans.firstOrNull()?.run
    val pt = first?.sizePt ?: when (p.heading) {
        0 -> 28f
        1 -> 16f
        2 -> 13f
        null -> 11f
        else -> 12f
    }
    return MaterialTheme.typography.bodyLarge.copy(
        color = INK,
        fontSize = (pt * SP_PER_PT).sp,
        lineHeight = 1.3.em,
        fontFamily = fontFamily(first?.font),
        fontWeight = if (first?.bold ?: (p.heading != null)) FontWeight.Bold else FontWeight.Normal,
        textAlign = when (p.align) {
            DocAlign.LEFT -> TextAlign.Start
            DocAlign.CENTER -> TextAlign.Center
            DocAlign.RIGHT -> TextAlign.End
            DocAlign.JUSTIFY -> TextAlign.Justify
        },
    )
}

@Composable
private fun FormatBar(s: WordSession, pages: Boolean, onPages: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainer)
            .horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FilterChip(
            pages, { onPages(!pages) }, { Text("Pages") },
            leadingIcon = { Icon(painterResource(R.drawable.ic_pages), null, Modifier.size(18.dp)) },
        )
        val p = s.active
        if (!pages && p != null) FormatControls(s, p)
    }
}

/** Formatting for the selection in [p], or for all of [p] when nothing is selected. */
@Composable
private fun FormatControls(s: WordSession, p: WordPara) {
    val look = s.look(p)
    FilterChip(
        look.bold == true, { s.format(runFormat(bold = look.bold != true)) },
        { Text("B", fontWeight = FontWeight.Bold) }, Modifier.semantics { contentDescription = "Bold" },
    )
    FilterChip(
        look.italic == true, { s.format(runFormat(italic = look.italic != true)) },
        { Text("I", fontStyle = FontStyle.Italic) }, Modifier.semantics { contentDescription = "Italic" },
    )
    FilterChip(
        look.underline == true, { s.format(runFormat(underline = look.underline != true)) },
        { Text("U", textDecoration = TextDecoration.Underline) }, Modifier.semantics { contentDescription = "Underline" },
    )
    FilterChip(
        look.strike == true, { s.format(runFormat(strike = look.strike != true)) },
        { Text("S", textDecoration = TextDecoration.LineThrough) }, Modifier.semantics { contentDescription = "Strikethrough" },
    )
    SizeMenu(look.sizePt) { s.format(runFormat(sizePt = it)) }
    ColorMenu { s.format(runFormat(color = it)) }
    AlignMenu(p.align, s::align)
    StyleMenu(s.styles, p.styleName, s::style)
    TextButton(s::pageBreak) { Text("Page break") }
    TextButton(s::done) { Text("Done") }
}

@Composable
private fun SizeMenu(size: Float?, onPick: (Float) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton({ open = true }) { Text(size?.let { "${pt(it)} pt" } ?: "Size") }
        DropdownMenu(open, { open = false }) {
            SIZES.forEach { s ->
                DropdownMenuItem(text = { Text("${pt(s)} pt") }, onClick = { open = false; onPick(s) })
            }
        }
    }
}

@Composable
private fun ColorMenu(onPick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton({ open = true }) { Text("Colour") }
        DropdownMenu(open, { open = false }) {
            COLORS.forEach { (hex, name) ->
                DropdownMenuItem(
                    text = { Text(name) },
                    leadingIcon = {
                        Box(
                            Modifier.size(20.dp).clip(CircleShape)
                                .background(argbOf(hex)?.let { Color(it) } ?: INK)
                                .border(1.dp, MaterialTheme.colorScheme.outline, CircleShape),
                        )
                    },
                    onClick = { open = false; onPick(hex) },
                )
            }
        }
    }
}

@Composable
private fun AlignMenu(current: DocAlign, onPick: (DocAlign) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton({ open = true }) { Text(ALIGNS.first { it.first == current }.second) }
        DropdownMenu(open, { open = false }) {
            ALIGNS.forEach { (align, name) ->
                DropdownMenuItem(text = { Text(name) }, onClick = { open = false; onPick(align) })
            }
        }
    }
}

@Composable
private fun StyleMenu(styles: List<DocStyle>, current: String?, onPick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton({ open = true }, enabled = styles.isNotEmpty()) { Text(current ?: "Style") }
        DropdownMenu(open, { open = false }) {
            styles.forEach { style ->
                DropdownMenuItem(text = { Text(style.name) }, onClick = { open = false; onPick(style.id) })
            }
        }
    }
}

private fun pt(size: Float): String = if (size % 1f == 0f) size.toInt().toString() else size.toString()

/** The document as its pages draw, read-only, to check the layout; drawn again after every change. */
@Composable
private fun WordPages(edit: EditScope<WordDoc>) {
    var count by remember { mutableStateOf<Int?>(null) }
    LaunchedEffect(edit.version) {
        count = try {
            edit.read { editor.pageCount().toInt() }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            edit.message(e.message ?: "Papercraft can't draw the pages of this document.")
            0
        }
    }
    // The core draws at a scale, not a width; the last page's width sets the next scale.
    val natural = remember { floatArrayOf(PAGE_PX) }
    val n = count ?: return Box(Modifier.fillMaxSize(), Alignment.Center) {
        CircularProgressIndicator(Modifier.semantics { contentDescription = "Loading" })
    }
    LazyColumn(
        Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerHighest),
        contentPadding = PaddingValues(12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        items(n) { i -> PageImage(edit, i, natural) }
    }
}

@Composable
private fun PageImage(edit: EditScope<WordDoc>, index: Int, natural: FloatArray) {
    var image by remember { mutableStateOf<ImageBitmap?>(null) }
    var width by remember { mutableIntStateOf(0) }
    LaunchedEffect(index, width, edit.version) {
        if (width <= 0) return@LaunchedEffect
        val scale = (width / natural[0]).coerceIn(0.1f, 4f)
        val png: ByteArray? = try {
            edit.read { editor.renderPage(index.toUInt(), scale) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            null
        }
        if (png == null) return@LaunchedEffect
        val bitmap = withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(png, 0, png.size) } ?: return@LaunchedEffect
        natural[0] = bitmap.width / scale
        image = bitmap.asImageBitmap()
    }
    val shown = image
    Box(
        Modifier.fillMaxWidth()
            .aspectRatio(shown?.let { it.width.toFloat() / it.height } ?: PAGE_RATIO)
            .background(PAPER)
            .onSizeChanged { width = it.width }
            .semantics { contentDescription = "Page ${index + 1}" },
    ) {
        shown?.let { Image(it, null, Modifier.fillMaxSize()) }
    }
}
