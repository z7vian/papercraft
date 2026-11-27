package org.papercraft

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SecondaryScrollableTabRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.papercraft.core.OfficeFamily
import org.papercraft.core.SheetEditHAlign
import org.papercraft.core.SheetEditVAlign
import org.papercraft.core.SheetEditor
import org.papercraft.core.SheetNumberFormat
import org.papercraft.core.SheetStyleEdit
import java.util.Locale

/** An Excel workbook in the edit shell. [converted]: it was a legacy .xls/.et file, which saves as .xlsx. */
class SheetDocEdit(val editor: SheetEditor, override val converted: Boolean) : EditableDoc {
    override fun undo() = editor.undo()
    override fun redo() = editor.redo()
    override fun canUndo() = editor.canUndo()
    override fun canRedo() = editor.canRedo()
    override fun save() = editor.save()
    override fun close() = editor.close()
}

/** The Excel editor: .xlsx and its kin, and old .xls/.xlt/.et/.ett (converted, saved as .xlsx). */
@Composable
fun SheetEditScreen(file: DocFile, restore: Boolean, onClose: () -> Unit) =
    EditShell(file, restore, onClose, open = { SheetDocEdit(SheetEditor.open(it, OfficeFamily.SPREADSHEET), converted = !isZip(it)) }) {
        SheetEditing(it)
    }

/** OOXML files are zip archives; anything else goes through the legacy converter, as the core decides. */
private fun isZip(bytes: ByteArray) = bytes.size >= 2 && bytes[0] == 'P'.code.toByte() && bytes[1] == 'K'.code.toByte()

/** How long typing into cells waits before the shown workbook reloads, so a run of entries costs one reload. */
private const val CELL_RELOAD_MS = 1_000L

/** Excel's limits: column width in characters, row height in points. */
private const val MAX_COL_WIDTH = 255.0
private const val MAX_ROW_HEIGHT = 409.0

private val NO_MERGES = GridMerges(emptyList())
private val A1 = SheetSelection(0, 0, GridRect(0, 0, 0, 0))

/** Excel's standard colours, as RRGGBB with their names. */
private val SHEET_COLORS = listOf(
    "000000" to "Black", "FFFFFF" to "White", "C00000" to "Dark red", "FF0000" to "Red", "FFC000" to "Orange",
    "FFFF00" to "Yellow", "92D050" to "Light green", "00B050" to "Green", "00B0F0" to "Light blue",
    "0070C0" to "Blue", "002060" to "Dark blue", "7030A0" to "Purple",
)

private val NUMBER_FORMATS = listOf(
    SheetNumberFormat.General to "General", SheetNumberFormat.Number to "Number", SheetNumberFormat.Currency to "Currency",
    SheetNumberFormat.Percent to "Percent", SheetNumberFormat.Scientific to "Scientific", SheetNumberFormat.Date to "Date",
    SheetNumberFormat.Time to "Time", SheetNumberFormat.Text to "Text",
)

private val SelectionSaver = listSaver<SheetSelection, Int>(
    save = { listOf(it.row, it.col, it.range.firstRow, it.range.firstCol, it.range.lastRow, it.range.lastCol, it.tabCol ?: -1) },
    restore = { SheetSelection(it[0], it[1], GridRect(it[2], it[3], it[4], it[5]), it[6].takeIf { c -> c >= 0 }) },
)

/** Which edit the shown workbook holds, and what to do once the next reload lands. Read and written on the main thread. */
private class SheetReload {
    /** The [EditScope.version] the shown workbook was saved at. */
    var shown = -1

    /** The version a cell entry made: its text already shows, so the reload can wait for a pause. */
    var cellEdit = -1

    /** The sheet to show after the next reload: one just added, moved or next to one deleted. */
    var nextSheet: Int? = null
}

private class SizeAsk(val column: Boolean, val current: Double?)

/**
 * The display path: the grid draws a [SheetGridBook] opened from the editor's own save, so formatting, sizes, sheets and
 * recalculated values all show exactly as the viewer shows the saved file. Typing shows at once through [SheetOverlay]
 * (the cells set_cell reports, recalculated ones included) and the reload waits for a pause; every other change, undo and
 * redo reloads right away.
 */
// Limit: each reload saves and re-reads the whole workbook, which takes seconds on very large files; a SheetEditor call
// that returns a window of cells with their view styles lifts it.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SheetEditing(edit: EditScope<SheetDocEdit>) {
    val scope = rememberCoroutineScope()
    var book by remember { mutableStateOf<SheetGridBook?>(null) }
    var failed by remember { mutableStateOf<String?>(null) }
    val overlay = remember { SheetOverlay() }
    var overlayVersion by remember { mutableIntStateOf(0) }
    val reload = remember { SheetReload() }
    var sheet by rememberSaveable { mutableIntStateOf(-1) }
    var selection by rememberSaveable(stateSaver = SelectionSaver) { mutableStateOf(A1) }
    val zoom = rememberSaveable { mutableFloatStateOf(1f) }
    val scrolls = remember { HashMap<Int, SheetEditScroll>() }
    val field = remember { CellField() }
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current

    LaunchedEffect(edit.version) {
        val target = edit.version
        if (target == reload.shown) return@LaunchedEffect
        if (target == reload.cellEdit && book != null) delay(CELL_RELOAD_MS)
        try {
            val bytes = edit.read { save() }
            // Once open, the new workbook is shown even if another edit comes in meanwhile, so it is never lost unclosed.
            withContext(NonCancellable) {
                val fresh = withContext(Dispatchers.IO) { SheetGridBook.open(bytes) }
                reload.shown = target
                overlay.shown(target)
                overlayVersion++
                book = fresh
                failed = null
                val want = reload.nextSheet ?: sheet
                reload.nextSheet = null
                val next = if (fresh.tabs.getOrNull(want)?.hidden == false) want else fresh.firstSheet
                if (next != sheet) {
                    sheet = next
                    selection = A1
                }
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            val text = e.message ?: "Papercraft can't show this spreadsheet."
            if (book == null) failed = text else edit.message(text)
        }
    }
    val shownBook = book
    DisposableEffect(shownBook) { onDispose { shownBook?.close() } }

    // The book and sheet stay up until the next ones are ready, so reloads and sheet changes don't flash.
    val loaded by produceState<Pair<SheetGridBook, Result<GridSheet>>?>(null, shownBook, sheet) {
        val b = shownBook ?: return@produceState
        if (sheet < 0) return@produceState
        val index = sheet
        value = b to withContext(Dispatchers.IO) { runCatching { b.sheet(index) } }
    }
    val grid = loaded?.second?.getOrNull()
    val merges = grid?.merges ?: NO_MERGES
    val at = Triple(sheet, selection.row, selection.col)

    LaunchedEffect(at, edit.version) {
        if (sheet < 0) return@LaunchedEffect
        val content = try {
            edit.read { editor.getCell(at.first.toUInt(), at.second.toUInt(), at.third.toUInt()) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            return@LaunchedEffect
        }
        field.load(at, content.input)
    }
    val style by produceState<SheetStyleEdit?>(null, at, edit.version) {
        if (sheet < 0) return@produceState
        value = try {
            edit.read { editor.getCellStyle(at.first.toUInt(), at.second.toUInt(), at.third.toUInt()) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            null
        }
    }

    fun commit() {
        val (cell, text) = field.take() ?: return
        edit.change({ editor.setCell(cell.first.toUInt(), cell.second.toUInt(), cell.third.toUInt(), text) }) { changes ->
            overlay.add(changes, edit.version)
            overlayVersion++
            reload.cellEdit = edit.version
        }
    }
    SideEffect { edit.commitPending = { commit() } }

    fun select(next: SheetSelection) {
        if (next.row != selection.row || next.col != selection.col) commit()
        selection = next
    }
    fun move(way: SheetMove) {
        commit()
        selection = selection.moved(way, merges)
    }
    fun goToSheet(index: Int) {
        commit()
        sheet = index
        selection = A1
    }
    fun restyle(s: SheetStyleEdit) {
        commit()
        val (index, range) = sheet to selection.cellRange()
        edit.change({ editor.setCellStyle(index.toUInt(), range, s) })
    }
    fun numberFormat(format: SheetNumberFormat) {
        commit()
        val (index, range) = sheet to selection.cellRange()
        edit.change({ editor.setNumberFormat(index.toUInt(), range, format) })
    }
    fun insertRows(below: Boolean) {
        commit()
        val (index, r) = sheet to selection.range
        val count = selection.rowCount
        edit.change({ editor.insertRows(index.toUInt(), (if (below) r.lastRow + 1 else r.firstRow).toUInt(), count.toUInt()) })
    }
    fun insertColumns(right: Boolean) {
        commit()
        val (index, r) = sheet to selection.range
        val count = selection.colCount
        edit.change({ editor.insertColumns(index.toUInt(), (if (right) r.lastCol + 1 else r.firstCol).toUInt(), count.toUInt()) })
    }
    fun deleteRows() {
        commit()
        val (index, r) = sheet to selection.range
        val count = selection.rowCount
        edit.change({ editor.deleteRows(index.toUInt(), r.firstRow.toUInt(), count.toUInt()) }) { selection = SheetSelection.at(r.firstRow, r.firstCol, NO_MERGES) }
    }
    fun deleteColumns() {
        commit()
        val (index, r) = sheet to selection.range
        val count = selection.colCount
        edit.change({ editor.deleteColumns(index.toUInt(), r.firstCol.toUInt(), count.toUInt()) }) { selection = SheetSelection.at(r.firstRow, r.firstCol, NO_MERGES) }
    }

    var sizing by remember { mutableStateOf<SizeAsk?>(null) }
    fun askSize(column: Boolean) {
        commit()
        val (index, sel) = sheet to selection
        scope.launch {
            try {
                val current = edit.read {
                    if (column) editor.columnWidth(index.toUInt(), sel.col.toUInt()) else editor.rowHeight(index.toUInt(), sel.row.toUInt())
                }
                sizing = SizeAsk(column, current)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Throwable) {
                edit.message(e.message ?: "Papercraft can't read that size.")
            }
        }
    }
    // Limit: a size set on several columns or rows is one undo step per track; a range call in the core lifts it.
    fun setSize(column: Boolean, value: Double?) {
        val (index, r) = sheet to selection.range
        edit.change({
            if (column) {
                for (c in r.firstCol..r.lastCol) editor.setColumnWidth(index.toUInt(), c.toUInt(), value)
            } else {
                for (row in r.firstRow..r.lastRow) editor.setRowHeight(index.toUInt(), row.toUInt(), value)
            }
            Unit
        })
    }

    val names = shownBook?.tabs?.map { it.name }.orEmpty()
    val tabs = shownBook?.let { b -> b.tabs.indices.filter { !b.tabs[it].hidden } }.orEmpty()
    var renaming by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }
    fun addSheet() {
        commit()
        val index = names.size
        val name = newSheetName(names)
        edit.change({ editor.addSheet(index.toUInt(), name) }) { reload.nextSheet = index }
    }
    fun moveSheet(to: Int) {
        commit()
        val from = sheet
        edit.change({ editor.moveSheet(from.toUInt(), to.toUInt()) }) { reload.nextSheet = to }
    }
    fun deleteSheet() {
        commit()
        val index = sheet
        val pos = tabs.indexOf(index)
        // The visible tab before it, or the one after, which moves down a place.
        val next = if (pos > 0) tabs[pos - 1] else tabs.getOrNull(1)?.minus(1) ?: 0
        edit.change({ editor.deleteSheet(index.toUInt()) }) { reload.nextSheet = next }
    }

    Column(Modifier.fillMaxSize()) {
        // The formula bar: the cell's address and what was typed into it, formulas included.
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                selectionName(selection, merges), Modifier.widthIn(min = 56.dp),
                style = MaterialTheme.typography.labelLarge, maxLines = 1,
            )
            OutlinedTextField(
                field.value, { field.value = it },
                Modifier.weight(1f).focusRequester(focus).semantics { contentDescription = "Cell contents" }
                    .onPreviewKeyEvent { e ->
                        val way = when (e.key) {
                            Key.Enter, Key.NumPadEnter -> if (e.isShiftPressed) SheetMove.Up else SheetMove.Down
                            Key.Tab -> if (e.isShiftPressed) SheetMove.Left else SheetMove.Right
                            Key.Escape -> null
                            else -> return@onPreviewKeyEvent false
                        }
                        if (e.type == KeyEventType.KeyDown) {
                            if (way == null) field.revert() else move(way)
                        }
                        true
                    },
                enabled = grid != null,
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { move(SheetMove.Down) }),
            )
        }

        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val on = grid != null
            FilterChip(
                style?.bold == true, { restyle(SheetStyleEdit(bold = style?.bold != true)) },
                { Text("B", fontWeight = FontWeight.Bold) }, Modifier.semantics { contentDescription = "Bold" }, enabled = on,
            )
            FilterChip(
                style?.italic == true, { restyle(SheetStyleEdit(italic = style?.italic != true)) },
                { Text("I", fontStyle = FontStyle.Italic) }, Modifier.semantics { contentDescription = "Italic" }, enabled = on,
            )
            SheetColorMenu("Text colour", on) { restyle(SheetStyleEdit(textColor = "#$it")) }
            SheetColorMenu("Fill", on) { restyle(SheetStyleEdit(fillColor = "#$it")) }
            SheetMenu(
                "Align", on,
                listOf(
                    "Left" to { restyle(SheetStyleEdit(horizontal = SheetEditHAlign.LEFT)) },
                    "Centre" to { restyle(SheetStyleEdit(horizontal = SheetEditHAlign.CENTER)) },
                    "Right" to { restyle(SheetStyleEdit(horizontal = SheetEditHAlign.RIGHT)) },
                    "Top" to { restyle(SheetStyleEdit(vertical = SheetEditVAlign.TOP)) },
                    "Middle" to { restyle(SheetStyleEdit(vertical = SheetEditVAlign.MIDDLE)) },
                    "Bottom" to { restyle(SheetStyleEdit(vertical = SheetEditVAlign.BOTTOM)) },
                ),
                dividerAfter = 2,
            )
            FilterChip(style?.wrap == true, { restyle(SheetStyleEdit(wrap = style?.wrap != true)) }, { Text("Wrap") }, enabled = on)
            SheetMenu(
                style?.numberFormat?.let { f -> NUMBER_FORMATS.firstOrNull { it.first == f }?.second ?: "Custom" } ?: "Format", on,
                NUMBER_FORMATS.map { (format, name) -> name to { numberFormat(format) } },
            )
            val rows = selection.rowCount
            val cols = selection.colCount
            SheetMenu(
                "Insert", on,
                listOf(
                    (if (rows == 1) "Row above" else "$rows rows above") to { insertRows(below = false) },
                    (if (rows == 1) "Row below" else "$rows rows below") to { insertRows(below = true) },
                    (if (cols == 1) "Column left" else "$cols columns left") to { insertColumns(right = false) },
                    (if (cols == 1) "Column right" else "$cols columns right") to { insertColumns(right = true) },
                ),
                dividerAfter = 1,
            )
            SheetMenu(
                "Delete", on,
                listOf(
                    (if (rows == 1) "Row" else "$rows rows") to { deleteRows() },
                    (if (cols == 1) "Column" else "$cols columns") to { deleteColumns() },
                ),
            )
            SheetMenu("Size", on, listOf("Column width…" to { askSize(column = true) }, "Row height…" to { askSize(column = false) }))
        }

        Box(Modifier.weight(1f).fillMaxWidth()) {
            val result = loaded
            when {
                failed != null && result == null -> Text(
                    failed.orEmpty(), Modifier.align(Alignment.Center).padding(32.dp),
                    textAlign = TextAlign.Center, style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                result == null -> CircularProgressIndicator(Modifier.align(Alignment.Center).semantics { contentDescription = "Loading" })
                grid == null -> Text(
                    result.second.exceptionOrNull()?.message ?: "This sheet can't be shown.",
                    Modifier.align(Alignment.Center).padding(32.dp),
                    textAlign = TextAlign.Center, style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> SheetEditGrid(
                    result.first, grid, zoom, scrolls.getOrPut(grid.index) { SheetEditScroll() }, selection, overlay, overlayVersion,
                    onSelect = { select(it) },
                    onEditCell = {
                        focus.requestFocus()
                        keyboard?.show()
                    },
                )
            }
        }

        Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                var menuOpen by remember { mutableStateOf(false) }
                // A tab row with no tabs crashes while measuring, so it waits for the workbook to load.
                if (tabs.isEmpty()) Spacer(Modifier.weight(1f)) else SecondaryScrollableTabRow(
                    selectedTabIndex = tabs.indexOf(sheet).coerceAtLeast(0),
                    modifier = Modifier.weight(1f),
                    containerColor = Color.Transparent,
                    edgePadding = 0.dp,
                ) {
                    val pos = tabs.indexOf(sheet)
                    for (index in tabs) {
                        Box {
                            Tab(
                                selected = index == sheet,
                                // Tapping the open sheet's tab opens its menu.
                                onClick = { if (index == sheet) menuOpen = true else goToSheet(index) },
                                text = { Text(names[index], maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            )
                            DropdownMenu(menuOpen && index == sheet, { menuOpen = false }) {
                                DropdownMenuItem(text = { Text("Rename…") }, onClick = { menuOpen = false; commit(); renaming = true })
                                DropdownMenuItem(
                                    text = { Text("Move left") }, enabled = pos > 0,
                                    onClick = { menuOpen = false; moveSheet(tabs[pos - 1]) },
                                )
                                DropdownMenuItem(
                                    text = { Text("Move right") }, enabled = pos in 0 until tabs.size - 1,
                                    onClick = { menuOpen = false; moveSheet(tabs[pos + 1]) },
                                )
                                DropdownMenuItem(
                                    text = { Text("Delete") }, enabled = tabs.size > 1,
                                    onClick = { menuOpen = false; deleting = true },
                                )
                            }
                        }
                    }
                }
                IconButton({ addSheet() }, enabled = shownBook != null) { Icon(painterResource(R.drawable.ic_add), "Add sheet") }
                IconButton({ zoom.floatValue = (zoom.floatValue / 1.5f).coerceAtLeast(EDIT_MIN_ZOOM) }, enabled = zoom.floatValue > EDIT_MIN_ZOOM) {
                    Icon(painterResource(R.drawable.ic_zoom_out), "Zoom out")
                }
                IconButton({ zoom.floatValue = (zoom.floatValue * 1.5f).coerceAtMost(EDIT_MAX_ZOOM) }, enabled = zoom.floatValue < EDIT_MAX_ZOOM) {
                    Icon(painterResource(R.drawable.ic_zoom_in), "Zoom in")
                }
            }
        }
    }

    if (renaming) {
        val self = sheet
        var name by remember { mutableStateOf(TextFieldValue(names.getOrElse(self) { "" }, TextRange(0, names.getOrElse(self) { "" }.length))) }
        val problem = sheetNameProblem(name.text, names, self)
        AlertDialog(
            onDismissRequest = { renaming = false },
            title = { Text("Rename sheet") },
            text = {
                OutlinedTextField(
                    name, { name = it }, singleLine = true, label = { Text("Name") },
                    isError = problem != null && name.text.isNotEmpty(),
                    supportingText = problem?.takeIf { name.text.isNotEmpty() }?.let { text -> @Composable { Text(text) } },
                )
            },
            confirmButton = {
                TextButton({
                    renaming = false
                    val newName = name.text
                    edit.change({ editor.renameSheet(self.toUInt(), newName) })
                }, enabled = problem == null) { Text("Rename") }
            },
            dismissButton = { TextButton({ renaming = false }) { Text("Cancel") } },
        )
    }
    if (deleting) {
        AlertDialog(
            onDismissRequest = { deleting = false },
            title = { Text("Delete sheet?") },
            text = { Text("\"${names.getOrElse(sheet) { "" }}\" and everything on it will be deleted. Undo brings it back.") },
            confirmButton = { TextButton({ deleting = false; deleteSheet() }) { Text("Delete") } },
            dismissButton = { TextButton({ deleting = false }) { Text("Cancel") } },
        )
    }
    sizing?.let { ask -> SizeDialog(ask, onDismiss = { sizing = null }) { value -> sizing = null; setSize(ask.column, value) } }
}

/** Asks for a column width (characters) or row height (points); [onSet] gets null to go back to the sheet's default. */
@Composable
private fun SizeDialog(ask: SizeAsk, onDismiss: () -> Unit, onSet: (Double?) -> Unit) {
    val max = if (ask.column) MAX_COL_WIDTH else MAX_ROW_HEIGHT
    var text by remember { mutableStateOf(ask.current?.let(::trimNumber).orEmpty()) }
    val value = text.trim().replace(',', '.').toDoubleOrNull()?.takeIf { it in 0.0..max }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (ask.column) "Column width" else "Row height") },
        text = {
            OutlinedTextField(
                text, { text = it }, singleLine = true,
                label = { Text(if (ask.column) "Width in characters" else "Height in points") },
                placeholder = { Text("Default") },
                isError = text.isNotBlank() && value == null,
                supportingText = { Text("0 to ${trimNumber(max)}; 0 hides it.") },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
            )
        },
        confirmButton = { TextButton({ onSet(value) }, enabled = value != null) { Text("Set") } },
        dismissButton = {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TextButton(onDismiss) { Text("Cancel") }
                TextButton({ onSet(null) }) { Text("Default") }
            }
        },
    )
}

private fun trimNumber(value: Double): String =
    if (value % 1.0 == 0.0) value.toLong().toString() else String.format(Locale.ROOT, "%.2f", value).trimEnd('0').trimEnd('.')

@Composable
private fun SheetMenu(label: String, enabled: Boolean, items: List<Pair<String, () -> Unit>>, dividerAfter: Int? = null) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton({ open = true }, enabled = enabled) { Text(label) }
        DropdownMenu(open, { open = false }) {
            items.forEachIndexed { i, (name, action) ->
                DropdownMenuItem(text = { Text(name) }, onClick = { open = false; action() })
                if (i == dividerAfter) HorizontalDivider()
            }
        }
    }
}

@Composable
private fun SheetColorMenu(label: String, enabled: Boolean, onPick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton({ open = true }, enabled = enabled) { Text(label) }
        DropdownMenu(open, { open = false }) {
            SHEET_COLORS.forEach { (hex, name) ->
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
 * The formula bar's text over a cell's input the engine holds: it shows the engine's input until the user types, then
 * keeps the typing until [take] hands it over. [owner] is the cell as (sheet, row, column).
 */
private class CellField {
    var value by mutableStateOf(TextFieldValue())
    private var owner: Triple<Int, Int, Int>? = null
    private var sent = ""

    fun load(owner: Triple<Int, Int, Int>, input: String) {
        if (owner != this.owner) {
            this.owner = owner
            value = TextFieldValue(input, TextRange(input.length))
            sent = input
        } else if (value.text == sent) {
            value = value.copy(text = input)
            sent = input
        }
    }

    /** Drops the typing: the cell's input shows again. */
    fun revert() {
        value = TextFieldValue(sent, TextRange(sent.length))
    }

    /** The typing the engine hasn't got, as (cell, text), now counted as sent; null when there is none. */
    fun take(): Pair<Triple<Int, Int, Int>, String>? {
        val o = owner ?: return null
        if (value.text == sent) return null
        sent = value.text
        return o to sent
    }
}
