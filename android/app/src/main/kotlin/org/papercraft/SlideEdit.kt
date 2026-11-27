package org.papercraft

import android.graphics.BitmapFactory
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.papercraft.core.DeckModel
import org.papercraft.core.DeckParagraph
import org.papercraft.core.DeckRect
import org.papercraft.core.DeckShape
import org.papercraft.core.DeckShapeKind
import org.papercraft.core.DeckTextStyle
import org.papercraft.core.OfficeFamily
import org.papercraft.core.SlideEditor

/** A PowerPoint deck in the edit shell. */
class SlideDoc(val editor: SlideEditor) : EditableDoc {
    override val converted = editor.converted()
    override fun undo() = editor.undo()
    override fun redo() = editor.redo()
    override fun canUndo() = editor.canUndo()
    override fun canRedo() = editor.canRedo()
    override fun save() = editor.save()
    override fun close() = editor.close()
}

/** The PowerPoint editor: .pptx and its kin, and old .ppt/.pps/.pot/.dps/.dpt (converted, saved as .pptx). */
@Composable
fun SlideEditScreen(file: DocFile, restore: Boolean, onClose: () -> Unit) =
    EditShell(file, restore, onClose, open = { SlideDoc(SlideEditor.open(it, OfficeFamily.PRESENTATION)) }) { SlideEditing(it) }

private const val EMU_PER_PX = 9525.0 // 914400 EMU per inch at 96 dpi, the core's scale 1.0
private const val THUMB_PX = 192
private const val TYPING_PAUSE_MS = 700L
private val SIZES = listOf(8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 32, 36, 40, 44, 48, 54, 60, 66, 72, 80, 88, 96)

/** PowerPoint's standard colours, as RRGGBB with their names. */
private val COLORS = listOf(
    "000000" to "Black", "FFFFFF" to "White", "C00000" to "Dark red", "FF0000" to "Red", "FFC000" to "Orange",
    "FFFF00" to "Yellow", "92D050" to "Light green", "00B050" to "Green", "00B0F0" to "Light blue",
    "0070C0" to "Blue", "002060" to "Dark blue", "7030A0" to "Purple",
)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SlideEditing(edit: EditScope<SlideDoc>) {
    var deck by remember { mutableStateOf<DeckModel?>(null) }
    LaunchedEffect(edit.version) {
        try {
            deck = edit.read { editor.deck() }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            edit.message(e.message ?: "Papercraft can't read these slides.")
        }
    }
    val model = deck ?: return Box(Modifier.fillMaxSize(), Alignment.Center) {
        CircularProgressIndicator(Modifier.semantics { contentDescription = "Loading" })
    }

    var current by rememberSaveable { mutableIntStateOf(0) }
    val index = current.coerceIn(0, (model.slides.size - 1).coerceAtLeast(0))
    val slide = model.slides.getOrNull(index)
    var shapeId by remember { mutableStateOf<String?>(null) }
    val shape = slide?.shapes?.firstOrNull { it.id == shapeId }
    val text = remember { EngineText() }
    val notes = remember { EngineText() }

    // The fields show the engine's text after every reload, unless they hold typing it hasn't got yet.
    LaunchedEffect(shape) { if (shape == null) text.clear() else text.load(shape.id, fieldText(shapeText(shape))) }
    LaunchedEffect(slide?.id, slide?.notes) { if (slide == null) notes.clear() else notes.load(slide.id, slide.notes) }

    fun flushText() {
        text.take()?.let { (id, t) -> edit.change({ editor.setShapeText(id, engineText(t)) }) }
    }
    fun flushNotes() {
        notes.take()?.let { (id, t) -> edit.change({ editor.setNotes(id, t) }) }
    }
    fun flushAll() {
        flushText()
        flushNotes()
    }
    SideEffect { edit.commitPending = { flushAll() } }
    // Typing goes to the engine after a pause, as one undo step.
    LaunchedEffect(text.value.text) {
        delay(TYPING_PAUSE_MS)
        flushText()
    }
    LaunchedEffect(notes.value.text) {
        delay(TYPING_PAUSE_MS)
        flushNotes()
    }

    fun selectShape(id: String?) {
        flushText()
        shapeId = id
    }
    fun goTo(i: Int) {
        flushAll()
        shapeId = null
        current = i
    }
    fun format(style: DeckTextStyle) {
        val id = shapeId ?: return
        flushText()
        val (start, end) = formatRange(text.value.selection.min, text.value.selection.max, text.value.text.length)
        edit.change({ editor.formatText(id, start.toUInt(), end.toUInt(), style) })
    }
    fun addSlide() {
        if (model.layouts.isEmpty()) return edit.message("This presentation has no slide layouts to add a slide with.")
        val at = if (slide == null) 0 else index + 1
        val layout = slide?.layoutIndex ?: 0u
        goTo(index)
        edit.change({ editor.addSlide(at.toUInt(), layout) }) { current = at }
    }
    fun duplicateSlide() {
        val id = slide?.id ?: return
        goTo(index)
        edit.change({ editor.duplicateSlide(id, (index + 1).toUInt()) }) { current = index + 1 }
    }
    fun deleteSlide() {
        val id = slide?.id ?: return
        goTo(index)
        edit.change({ editor.deleteSlide(id) }) { current = (index - 1).coerceAtLeast(0).takeIf { index == model.slides.size - 1 } ?: index }
    }
    fun moveSlide(to: Int) {
        val id = slide?.id ?: return
        goTo(index)
        edit.change({ editor.moveSlide(id, to.toUInt()) }) { current = to }
    }
    fun addTextBox() {
        val id = slide?.id ?: return
        val box = DeckRect(
            x = model.widthEmu / 5, y = model.heightEmu * 2 / 5,
            width = model.widthEmu * 3 / 5, height = model.heightEmu / 6,
        )
        goTo(index)
        edit.change({ editor.addTextBox(id, box, "Text") }) { shapeId = it }
    }

    Column(Modifier.fillMaxSize()) {
        // The slide, as large as fits.
        Box(Modifier.weight(1f).fillMaxWidth().padding(12.dp), contentAlignment = Alignment.Center) {
            if (slide == null) {
                Text("This presentation has no slides.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else {
                SlideCanvas(edit, model, index, shape) { x, y -> selectShape(shapeAt(slide.shapes, x, y)?.id) }
            }
        }

        if (shape != null) {
            val selection = text.value.selection
            val (start, end) = formatRange(selection.min, selection.max, text.value.text.length)
            val style = rangeStyle(shape.paragraphs, start, end)
            Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp)) {
                OutlinedTextField(
                    text.value, { text.value = it }, Modifier.fillMaxWidth(),
                    label = { Text("Text") }, maxLines = 5,
                )
                Row(
                    Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(vertical = 4.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    FilterChip(
                        style.bold == true, { format(deckStyle(bold = style.bold != true)) },
                        { Text("B", fontWeight = FontWeight.Bold) }, Modifier.semantics { contentDescription = "Bold" },
                    )
                    FilterChip(
                        style.italic == true, { format(deckStyle(italic = style.italic != true)) },
                        { Text("I", fontStyle = FontStyle.Italic) }, Modifier.semantics { contentDescription = "Italic" },
                    )
                    FilterChip(
                        style.underline == true, { format(deckStyle(underline = style.underline != true)) },
                        { Text("U", textDecoration = TextDecoration.Underline) }, Modifier.semantics { contentDescription = "Underline" },
                    )
                    SizeMenu(style.sizePt) { format(deckStyle(sizePt = it)) }
                    ColorMenu { format(deckStyle(color = it)) }
                    TextButton({ selectShape(null) }) { Text("Done") }
                }
            }
        } else {
            if (slide != null) {
                Text(
                    "Tap text on the slide to edit it.",
                    Modifier.padding(horizontal = 16.dp),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            OutlinedTextField(
                notes.value, { notes.value = it }, Modifier.fillMaxWidth().padding(horizontal = 12.dp),
                enabled = slide != null, label = { Text("Notes") }, maxLines = 4,
            )
        }

        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TextButton({ addTextBox() }, enabled = slide != null) { Text("Text box") }
            TextButton({ addSlide() }) { Text("New slide") }
            TextButton({ duplicateSlide() }, enabled = slide != null) { Text("Duplicate") }
            TextButton({ deleteSlide() }, enabled = slide != null) { Text("Delete") }
            TextButton({ moveSlide(index - 1) }, enabled = slide != null && index > 0) { Text("Move left") }
            TextButton({ moveSlide(index + 1) }, enabled = slide != null && index < model.slides.size - 1) { Text("Move right") }
        }

        val strip = rememberLazyListState()
        LaunchedEffect(index) { if (model.slides.isNotEmpty()) strip.animateScrollToItem(index) }
        LazyRow(
            state = strip,
            contentPadding = PaddingValues(12.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            itemsIndexed(model.slides, key = { _, s -> s.id }) { i, _ ->
                SlideThumb(edit, model, i, selected = i == index) { goTo(i) }
            }
        }
    }
}

/** Slide [index] drawn to fit, with [selected]'s outline; a tap reports where it landed, in EMU. */
@Composable
private fun SlideCanvas(edit: EditScope<SlideDoc>, model: DeckModel, index: Int, selected: DeckShape?, onTap: (Long, Long) -> Unit) {
    var size by remember { mutableStateOf(IntSize.Zero) }
    val image = slideImage(edit, model, index, size.width)
    val outline = MaterialTheme.colorScheme.primary
    val tap by rememberUpdatedState(onTap)
    Box(
        Modifier.aspectRatio(model.ratio())
            .background(MaterialTheme.colorScheme.surfaceContainerHighest)
            .onSizeChanged { size = it }
            .pointerInput(model.widthEmu, model.heightEmu) {
                detectTapGestures { p ->
                    if (this.size.width > 0 && this.size.height > 0) {
                        tap((p.x / this.size.width * model.widthEmu).toLong(), (p.y / this.size.height * model.heightEmu).toLong())
                    }
                }
            }
            .drawWithContent {
                drawContent()
                val b = selected?.bounds ?: return@drawWithContent
                val k = this.size.width / model.widthEmu.toFloat()
                drawRect(
                    outline, Offset(b.x * k, b.y * k), Size(b.width * k, b.height * k),
                    style = Stroke(2.dp.toPx()),
                )
            }
            .semantics { contentDescription = "Slide ${index + 1}" },
    ) {
        image?.let { Image(it, null, Modifier.fillMaxSize()) }
    }
}

@Composable
private fun SlideThumb(edit: EditScope<SlideDoc>, model: DeckModel, index: Int, selected: Boolean, onClick: () -> Unit) {
    val image = slideImage(edit, model, index, THUMB_PX)
    val border = if (selected) BorderStroke(2.dp, MaterialTheme.colorScheme.primary) else BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant)
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Box(
            Modifier.width(96.dp).aspectRatio(model.ratio())
                .border(border)
                .background(MaterialTheme.colorScheme.surfaceContainerHighest)
                .clickable(onClick = onClick)
                .semantics {
                    contentDescription = "Slide ${index + 1}"
                    this.selected = selected
                },
        ) {
            image?.let { Image(it, null, Modifier.fillMaxSize()) }
        }
        Text("${index + 1}", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** Slide [index] drawn [widthPx] wide, again after every edit; the last drawing stays up meanwhile. */
@Composable
private fun slideImage(edit: EditScope<SlideDoc>, model: DeckModel, index: Int, widthPx: Int): ImageBitmap? {
    var image by remember { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(index, widthPx, edit.version) {
        if (widthPx <= 0) return@LaunchedEffect
        val scale = (widthPx / (model.widthEmu / EMU_PER_PX)).toFloat().coerceIn(0.05f, 4f)
        val png: ByteArray? = try {
            edit.read { editor.renderSlide(index.toUInt(), scale) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            null
        }
        if (png == null) return@LaunchedEffect
        val bitmap = withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(png, 0, png.size) } ?: return@LaunchedEffect
        image = bitmap.asImageBitmap()
    }
    return image
}

@Composable
private fun SizeMenu(size: Double?, onPick: (Double) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton({ open = true }) { Text(size?.let { "${pt(it)} pt" } ?: "Size") }
        DropdownMenu(open, { open = false }) {
            SIZES.forEach { s ->
                DropdownMenuItem(text = { Text("$s pt") }, onClick = { open = false; onPick(s.toDouble()) })
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
                                .background(Color(0xFF000000L or hex.toLong(16)))
                                .border(1.dp, MaterialTheme.colorScheme.outline, CircleShape),
                        )
                    },
                    onClick = { open = false; onPick(hex) },
                )
            }
        }
    }
}

/**
 * A text field over text the engine holds: it shows the engine's text until the user types, then keeps the typing
 * until [take] hands it over. [owner] is the shape or slide the text belongs to.
 */
private class EngineText {
    var value by mutableStateOf(TextFieldValue())
    private var owner: String? = null
    private var sent = ""

    fun load(owner: String, text: String) {
        if (owner != this.owner) {
            this.owner = owner
            value = TextFieldValue(text, TextRange(text.length))
            sent = text
        } else if (value.text == sent) {
            value = value.copy(text = text)
            sent = text
        }
    }

    fun clear() {
        owner = null
        value = TextFieldValue()
        sent = ""
    }

    /** The typing the engine hasn't got, as (owner, text), now counted as sent; null when there is none. */
    fun take(): Pair<String, String>? {
        val o = owner ?: return null
        if (value.text == sent) return null
        sent = value.text
        return o to sent
    }
}

private fun DeckModel.ratio(): Float = if (heightEmu > 0 && widthEmu > 0) widthEmu.toFloat() / heightEmu else 16f / 9f

private fun pt(size: Double): String = if (size % 1.0 == 0.0) size.toLong().toString() else size.toString()

private fun deckStyle(
    bold: Boolean? = null,
    italic: Boolean? = null,
    underline: Boolean? = null,
    sizePt: Double? = null,
    color: String? = null,
) = DeckTextStyle(bold = bold, italic = italic, underline = underline, strike = null, sizePt = sizePt, color = color, font = null)

/** The shape's text as [SlideEditor.setShapeText] takes it: paragraphs joined with `\n`. */
internal fun shapeText(shape: DeckShape): String =
    shape.paragraphs.joinToString("\n") { p -> p.runs.joinToString("") { it.text } }

/**
 * The engine marks a soft line break with U+000B; the field shows U+2028 (line separator) instead. Both are one UTF-16
 * unit, so offsets in the field are offsets in the engine's text.
 * Limit: the field may draw U+2028 as a space rather than a break; a field that lays it out as a break lifts it.
 */
internal fun fieldText(engine: String): String = engine.replace('\u000B', '\u2028')

internal fun engineText(field: String): String = field.replace('\u2028', '\u000B')

/** The range formatting applies to: the selection [selStart]..[selEnd], or the whole text of [length] when nothing is selected. */
internal fun formatRange(selStart: Int, selEnd: Int, length: Int): Pair<Int, Int> =
    if (selStart == selEnd) 0 to length else minOf(selStart, selEnd).coerceIn(0, length) to maxOf(selStart, selEnd).coerceIn(0, length)

/**
 * The formatting every run in UTF-16 range [start, end) of [paragraphs] shares (paragraph breaks count one unit, as in
 * [shapeText]). A field is null where the runs differ or inherit it.
 */
internal fun rangeStyle(paragraphs: List<DeckParagraph>, start: Int, end: Int): DeckTextStyle {
    val styles = mutableListOf<DeckTextStyle>()
    var at = 0
    paragraphs.forEachIndexed { i, p ->
        if (i > 0) at++
        for (run in p.runs) {
            val runEnd = at + run.text.length
            if (runEnd > at && runEnd > start && at < end) styles += run.style
            at = runEnd
        }
    }
    fun <T> shared(field: (DeckTextStyle) -> T?): T? = styles.map(field).distinct().singleOrNull()
    return DeckTextStyle(
        bold = shared { it.bold }, italic = shared { it.italic }, underline = shared { it.underline },
        strike = shared { it.strike }, sizePt = shared { it.sizePt }, color = shared { it.color }, font = shared { it.font },
    )
}

/**
 * The topmost shape with text at ([x], [y]) EMU, or null.
 * Limit: rotation is ignored and members of a group can't be picked (they report group coordinates, see the core's
 * DeckShape.bounds); mapping through rotation and the group transform lifts it.
 */
internal fun shapeAt(shapes: List<DeckShape>, x: Long, y: Long): DeckShape? = shapes.lastOrNull { s ->
    (s.kind == DeckShapeKind.SHAPE || s.kind == DeckShapeKind.TEXT_BOX) && s.groupId == null &&
        x >= s.bounds.x && x < s.bounds.x + s.bounds.width && y >= s.bounds.y && y < s.bounds.y + s.bounds.height
}
