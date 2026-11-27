package org.papercraft

import android.graphics.Paint
import android.os.SystemClock
import androidx.compose.animation.core.AnimationState
import androidx.compose.animation.core.animateDecay
import androidx.compose.animation.splineBasedDecay
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableFloatState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.launch
import org.papercraft.core.SheetCell
import org.papercraft.core.SheetCellStyle
import org.papercraft.core.SheetHAlign
import org.papercraft.core.SheetVAlign
import org.papercraft.core.SheetValueKind
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max

internal const val EDIT_MIN_ZOOM = 0.5f
internal const val EDIT_MAX_ZOOM = 4f

/** Where an editor's sheet is scrolled, in sheet pixels past its frozen tracks; kept across reloads of the workbook. */
internal class SheetEditScroll {
    var x by mutableDoubleStateOf(0.0)
    var y by mutableDoubleStateOf(0.0)
}

/** What the pane draws: a sheet's shape with its loaded cells. A reload swaps it only once the new cells in view are in. */
private class EditLayer(val grid: GridSheet, val cells: GridCells, val painter: GridPainter, val headerW: Float)

/** The style a cell with none of its own draws with until the reload brings the sheet's. */
private fun plainStyle(kind: SheetValueKind) = SheetCellStyle(
    fontName = null, fontSizePt = 11f, bold = false, italic = false, underline = false, strike = false, fontColor = null, fill = null,
    hAlign = when (kind) {
        SheetValueKind.NUMBER -> SheetHAlign.RIGHT
        SheetValueKind.BOOL, SheetValueKind.ERROR -> SheetHAlign.CENTER
        else -> SheetHAlign.LEFT
    },
    vAlign = SheetVAlign.BOTTOM, wrap = false, borderLeft = null, borderRight = null, borderTop = null, borderBottom = null,
)

/**
 * The editor's cell grid: [grid] of [book] drawn as the viewer draws it, with [selection] on top. A tap selects a cell,
 * a second tap on it calls [onEditCell]; dragging the selection's corner handle, or holding a cell then dragging, selects
 * a block. One finger scrolls, two zoom. [overlay] text shows over cells the workbook doesn't hold yet; [overlayVersion]
 * bumps when it changes.
 */
// Limit: screen readers hear the sheet and the selected cell, not each cell; per-cell semantics for the visible window lift it.
@Composable
internal fun SheetEditGrid(
    book: SheetGridBook,
    grid: GridSheet,
    zoom: MutableFloatState,
    scroll: SheetEditScroll,
    selection: SheetSelection,
    overlay: SheetOverlay,
    overlayVersion: Int,
    onSelect: (SheetSelection) -> Unit,
    onEditCell: () -> Unit,
) {
    val density = LocalDensity.current
    val scheme = MaterialTheme.colorScheme
    val haptics = LocalHapticFeedback.current
    val headerTextPx = with(density) { 12.sp.toPx() }
    val headerH = max(with(density) { 24.dp.toPx() }, headerTextPx * 1.8f)
    val handleR = with(density) { 6.dp.toPx() }
    val handleTouch = with(density) { 28.dp.toPx() }
    val edgeZone = with(density) { 32.dp.toPx() }
    val edgeStep = with(density) { 12.dp.toPx() }
    var layer by remember { mutableStateOf<EditLayer?>(null) }
    var area by remember { mutableStateOf(IntSize.Zero) }
    val currentSelection by rememberUpdatedState(selection)
    val select by rememberUpdatedState(onSelect)
    val editCell by rememberUpdatedState(onEditCell)

    fun viewOf(g: GridSheet, headerW: Float) = GridViewport(
        g.cols, g.rows, g.layout.frozenCols.toInt(), g.layout.frozenRows.toInt(),
        density.density * zoom.floatValue, scroll.x, scroll.y, area.width.toFloat(), area.height.toFloat(), headerW, headerH,
    )

    fun viewport(): GridViewport? = layer?.let { viewOf(it.grid, it.headerW) }

    /** Scrolls by screen pixels; false when already at the edge both ways. The sheet scrolls to Excel's last cell. */
    fun scrollBy(dx: Float, dy: Float): Boolean {
        val v = viewport() ?: return false
        val x = (scroll.x + dx / v.scale).coerceIn(0.0, v.maxScrollX(SHEET_COLS))
        val y = (scroll.y + dy / v.scale).coerceIn(0.0, v.maxScrollY(SHEET_ROWS, 0f))
        val moved = x != scroll.x || y != scroll.y
        scroll.x = x
        scroll.y = y
        return moved
    }

    fun zoomBy(factor: Float, centroid: Offset) {
        val before = viewport() ?: return
        val old = zoom.floatValue
        val new = (old * factor).coerceIn(EDIT_MIN_ZOOM, EDIT_MAX_ZOOM)
        if (new == old) return
        zoom.floatValue = new
        val after = viewport() ?: return
        scroll.x = gridZoomScroll(scroll.x, centroid.x - before.bodyX, centroid.x - after.bodyX, before.scale, after.scale)
        scroll.y = gridZoomScroll(scroll.y, centroid.y - before.bodyY, centroid.y - after.bodyY, before.scale, after.scale)
        scrollBy(0f, 0f)
    }

    fun applyOverlay(l: EditLayer) {
        for ((at, text) in overlay.of(l.grid.index)) {
            val kind = displayKind(text)
            val style = l.cells.cell(at.row, at.col)?.style ?: plainStyle(kind)
            l.cells.put(SheetCell(at.row.toUInt(), at.col.toUInt(), text, kind, style))
        }
    }

    // A new grid (a reload, another sheet) loads the cells in view, then takes over from the old one, so edits don't flash.
    LaunchedEffect(grid, scheme, headerTextPx) {
        val cells = GridCells(book, grid)
        val painter = GridPainter(
            grid, cells,
            GridColors(
                paper = android.graphics.Color.WHITE,
                gridline = EXCEL_GRIDLINE,
                headerBg = scheme.surfaceContainerHigh.toArgb(),
                headerText = scheme.onSurfaceVariant.toArgb(),
                headerLine = scheme.outlineVariant.toArgb(),
                frozenLine = scheme.outline.toArgb(),
            ),
            headerTextPx,
        )
        // Room for row numbers a digit longer than the used range's, as rows get added below it.
        val digits = max(4, (grid.rowCount + 1).toString().length + 1)
        val headerW = Paint().apply { textSize = headerTextPx }.measureText("0".repeat(digits)) + with(density) { 16.dp.toPx() }
        val next = EditLayer(grid, cells, painter, headerW)
        snapshotFlow { if (area == IntSize.Zero) null else neededBlocks(viewOf(grid, headerW), grid) }
            .filterNotNull()
            .distinctUntilChanged()
            .collectLatest { keys ->
                cells.load(keys)
                applyOverlay(next)
                if (layer !== next) {
                    layer = next
                    scrollBy(0f, 0f)
                }
            }
    }
    LaunchedEffect(layer, overlayVersion) { layer?.let(::applyOverlay) }
    LaunchedEffect(zoom.floatValue, area) { scrollBy(0f, 0f) }

    // Keeps the active cell in view as Enter and Tab move it.
    LaunchedEffect(selection.row, selection.col, layer != null) {
        val l = layer ?: return@LaunchedEffect
        val v = viewOf(l.grid, l.headerW)
        val a = selection.active(l.grid.merges)
        if (a.firstCol >= v.frozenCols) {
            val origin = l.grid.cols.start(v.frozenCols)
            scroll.x = revealScroll(scroll.x, l.grid.cols.start(a.firstCol) - origin, l.grid.cols.start(a.lastCol + 1) - origin, ((v.width - v.bodyX) / v.scale).toDouble())
        }
        if (a.firstRow >= v.frozenRows) {
            val origin = l.grid.rows.start(v.frozenRows)
            scroll.y = revealScroll(scroll.y, l.grid.rows.start(a.firstRow) - origin, l.grid.rows.start(a.lastRow + 1) - origin, ((v.height - v.bodyY) / v.scale).toDouble())
        }
        scrollBy(0f, 0f)
    }

    val selectionFill = scheme.primary.copy(alpha = 0.12f).toArgb()
    val selectionLine = scheme.primary.toArgb()
    val paint = remember { Paint(Paint.ANTI_ALIAS_FLAG) }
    val name = book.tabs.getOrNull(grid.index)?.name.orEmpty()
    Canvas(
        Modifier.fillMaxSize()
            .onSizeChanged { area = it }
            .semantics { contentDescription = "Sheet $name, cell ${selectionName(selection, grid.merges)} selected" }
            // Restarts per sheet: each sheet keeps its own scroll.
            .pointerInput(scroll) {
                val decay = splineBasedDecay<Float>(this)
                coroutineScope {
                    val flings = this
                    var fling: Job? = null
                    var lastTapAt = 0L
                    var lastTapCell: SheetCellAt? = null
                    awaitEachGesture {
                        val down = awaitFirstDown(requireUnconsumed = false)
                        fling?.cancel()
                        val startedAt = SystemClock.uptimeMillis()
                        fun cellAt(p: Offset): SheetCellAt? {
                            val v = viewport() ?: return null
                            val r = v.rowAt(p.y) ?: return null
                            val c = v.colAt(p.x) ?: return null
                            return SheetCellAt(r.coerceIn(0, SHEET_ROWS - 1), c.coerceIn(0, SHEET_COLS - 1))
                        }
                        val tracker = VelocityTracker()
                        tracker.addPosition(down.uptimeMillis, down.position)
                        val onHandle = handleAt(viewport(), layer, currentSelection)?.let { (down.position - it).getDistance() <= handleTouch } == true
                        // Pending until the finger moves (scroll), holds still (select a block), lifts (tap) or a second one lands (zoom).
                        var mode = if (onHandle) DragMode.Extend else DragMode.Pending
                        var moved = Offset.Zero
                        var noX = false
                        var noY = false
                        while (true) {
                            val event = if (mode == DragMode.Pending) {
                                val left = viewConfiguration.longPressTimeoutMillis - (SystemClock.uptimeMillis() - startedAt)
                                withTimeoutOrNull(max(left, 1L)) { awaitPointerEvent() } ?: run {
                                    mode = DragMode.Extend
                                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                    val l = layer
                                    cellAt(down.position)?.let { if (l != null) select(SheetSelection.at(it.row, it.col, l.grid.merges)) }
                                    null
                                } ?: continue
                            } else awaitPointerEvent()
                            val pressed = event.changes.count { it.pressed }
                            if (pressed == 0) break
                            if (pressed >= 2 && mode != DragMode.Extend) {
                                mode = DragMode.Pinch
                                val z = event.calculateZoom()
                                if (z != 1f) zoomBy(z, event.calculateCentroid())
                                val pan = event.calculatePan()
                                scrollBy(-pan.x, -pan.y)
                                event.changes.forEach { it.consume() }
                                continue
                            }
                            val change = event.changes.firstOrNull { it.id == down.id } ?: continue
                            when (mode) {
                                DragMode.Pending -> {
                                    moved += event.calculatePan()
                                    if (moved.getDistance() > viewConfiguration.touchSlop) {
                                        mode = DragMode.Scroll
                                        // A drag that starts mostly along one axis stays on it, as spreadsheets scroll.
                                        noX = abs(moved.y) > 2 * abs(moved.x)
                                        noY = abs(moved.x) > 2 * abs(moved.y)
                                    }
                                }
                                DragMode.Scroll -> {
                                    val pan = event.calculatePan()
                                    scrollBy(if (noX) 0f else -pan.x, if (noY) 0f else -pan.y)
                                    tracker.addPosition(change.uptimeMillis, change.position)
                                    event.changes.forEach { it.consume() }
                                }
                                DragMode.Extend -> {
                                    val p = change.position
                                    // Near an edge the sheet scrolls on, so a block can reach past the screen.
                                    val dx = if (p.x > area.width - edgeZone) edgeStep else if (p.x < edgeZone + (layer?.headerW ?: 0f)) -edgeStep else 0f
                                    val dy = if (p.y > area.height - edgeZone) edgeStep else if (p.y < edgeZone + headerH) -edgeStep else 0f
                                    if (dx != 0f || dy != 0f) scrollBy(dx, dy)
                                    val l = layer
                                    val at = cellAt(p)
                                    if (l != null && at != null) select(currentSelection.extendTo(at.row, at.col, l.grid.merges))
                                    event.changes.forEach { it.consume() }
                                }
                                DragMode.Pinch -> event.changes.forEach { it.consume() }
                            }
                        }
                        when (mode) {
                            DragMode.Pending -> {
                                val l = layer
                                val at = cellAt(down.position)
                                if (l != null && at != null) {
                                    val now = SystemClock.uptimeMillis()
                                    val target = SheetSelection.at(at.row, at.col, l.grid.merges)
                                    val again = SheetCellAt(target.row, target.col)
                                    if (again == lastTapCell && now - lastTapAt < viewConfiguration.doubleTapTimeoutMillis) {
                                        editCell()
                                        lastTapCell = null
                                    } else {
                                        select(target)
                                        lastTapCell = again
                                        lastTapAt = now
                                    }
                                }
                            }
                            DragMode.Scroll -> {
                                val velocity = tracker.calculateVelocity()
                                val vx = if (noX) 0f else velocity.x
                                val vy = if (noY) 0f else velocity.y
                                val speed = hypot(vx, vy)
                                if (speed > 50f) {
                                    fling = flings.launch {
                                        var last = 0f
                                        AnimationState(0f, speed).animateDecay(decay) {
                                            val step = value - last
                                            last = value
                                            if (!scrollBy(-vx / speed * step, -vy / speed * step)) cancelAnimation()
                                        }
                                    }
                                }
                            }
                            else -> {}
                        }
                    }
                }
            },
    ) {
        val l = layer ?: return@Canvas
        l.cells.version // redraw as blocks arrive
        val v = viewOf(l.grid, l.headerW)
        drawIntoCanvas { canvas ->
            val c = canvas.nativeCanvas
            l.painter.draw(c, v)
            val range = selection.range
            val active = selection.active(l.grid.merges)
            val stroke = 2.dp.toPx()
            val single = range == active
            for (frozenRow in PANE_SIDES) {
                if (frozenRow && v.frozenRows == 0) continue
                for (frozenCol in PANE_SIDES) {
                    if (frozenCol && v.frozenCols == 0) continue
                    c.save()
                    c.clipRect(
                        if (frozenCol) v.headerW else v.bodyX, if (frozenRow) v.headerH else v.bodyY,
                        if (frozenCol) v.bodyX else v.width, if (frozenRow) v.bodyY else v.height,
                    )
                    val left = v.edgeX(range.firstCol, frozenCol)
                    val top = v.edgeY(range.firstRow, frozenRow)
                    val right = v.edgeX(range.lastCol + 1, frozenCol)
                    val bottom = v.edgeY(range.lastRow + 1, frozenRow)
                    if (!single) {
                        // The block is tinted; the active cell stays clear, as in Excel.
                        c.save()
                        c.clipOutRect(
                            v.edgeX(active.firstCol, frozenCol), v.edgeY(active.firstRow, frozenRow),
                            v.edgeX(active.lastCol + 1, frozenCol), v.edgeY(active.lastRow + 1, frozenRow),
                        )
                        paint.style = Paint.Style.FILL
                        paint.color = selectionFill
                        c.drawRect(left, top, right, bottom, paint)
                        c.restore()
                    }
                    paint.style = Paint.Style.STROKE
                    paint.strokeWidth = stroke
                    paint.color = selectionLine
                    c.drawRect(left, top, right, bottom, paint)
                    c.restore()
                }
            }
            handleAt(v, l, selection)?.let { h ->
                if (h.x >= v.headerW && h.y >= v.headerH) {
                    paint.style = Paint.Style.FILL
                    paint.color = selectionLine
                    c.drawCircle(h.x, h.y, handleR, paint)
                }
            }
        }
    }
}

private enum class DragMode { Pending, Scroll, Extend, Pinch }

/** Frozen pane, then scrolling body: the selection draws in each, clipped to it. */
private val PANE_SIDES = booleanArrayOf(true, false)

/** Screen x of column [col]'s left edge, placed as the frozen pane ([frozen]) or the scrolling body draws it. */
private fun GridViewport.edgeX(col: Int, frozen: Boolean): Float =
    if (frozen) headerW + (cols.start(col) * scale).toFloat()
    else bodyX + ((cols.start(col) - cols.start(frozenCols) - scrollX) * scale).toFloat()

/** Screen y of row [row]'s top edge, placed as the frozen pane ([frozen]) or the scrolling body draws it. */
private fun GridViewport.edgeY(row: Int, frozen: Boolean): Float =
    if (frozen) headerH + (rows.start(row) * scale).toFloat()
    else bodyY + ((rows.start(row) - rows.start(frozenRows) - scrollY) * scale).toFloat()

/** Where the selection's corner handle sits on screen: its bottom-right corner. */
private fun handleAt(v: GridViewport?, l: EditLayer?, selection: SheetSelection): Offset? {
    if (v == null || l == null) return null
    val r = selection.range
    return Offset(v.edgeX(r.lastCol + 1, r.lastCol < v.frozenCols), v.edgeY(r.lastRow + 1, r.lastRow < v.frozenRows))
}
