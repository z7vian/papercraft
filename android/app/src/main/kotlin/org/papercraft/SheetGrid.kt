package org.papercraft

import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.AnimationState
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.animateDecay
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.splineBasedDecay
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SecondaryScrollableTabRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableFloatState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollDispatcher
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

private const val GRID_MIN_ZOOM = 0.5f
private const val GRID_MAX_ZOOM = 4f

/** Excel's default gridline grey: part of how the sheet looks, like the white paper. */
private const val EXCEL_GRIDLINE = 0xFFD4D4D4.toInt()

/** Where a pane was scrolled, in sheet pixels past its frozen tracks. */
private class GridScroll(val x: Double, val y: Double)

/** A pending jump to a cell, from a link. */
private data class GridJump(val sheet: Int, val row: Int, val col: Int)

/**
 * A spreadsheet as Excel shows it: headers, frozen panes, merged cells and cell formatting, with sheet
 * tabs below. Cells load window by window as they scroll into view. Fills the screen under floating
 * bars like [PagedViewer]; [padding] keeps the headers clear of the top bar while it shows.
 */
// Limit: screen readers hear the sheet's name and size, not its cells; per-cell semantics for the visible window lift it.
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SheetGrid(book: SheetGridBook, padding: PaddingValues, chrome: Boolean, onTap: () -> Unit, onFirstPage: (Bitmap) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var sheet by rememberSaveable(book) { mutableIntStateOf(book.firstSheet) }
    // Shared by every sheet of the workbook.
    val zoom = rememberSaveable { mutableFloatStateOf(1f) }
    val scrolls = remember(book) { HashMap<Int, GridScroll>() }
    var jump by remember(book) { mutableStateOf<GridJump?>(null) }
    var thumbSaved by remember(book) { mutableStateOf(false) }
    var bottomBar by remember { mutableIntStateOf(0) }
    val tabs = remember(book) { book.tabs.indices.filter { !book.tabs[it].hidden }.ifEmpty { listOf(book.firstSheet) } }
    val cutout = WindowInsets.displayCutout.asPaddingValues().calculateTopPadding()
    val top by animateDpAsState(if (chrome) padding.calculateTopPadding() else cutout, tween(220), label = "gridTop")

    fun zoomTo(target: Float) = scope.launch {
        animate(zoom.floatValue, target.coerceIn(GRID_MIN_ZOOM, GRID_MAX_ZOOM), animationSpec = tween(240, easing = FastOutSlowInEasing)) { value, _ ->
            zoom.floatValue = value
        }
    }

    val onLink: (GridLinkTarget) -> Unit = { target ->
        when (target) {
            is GridLinkTarget.Web -> runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(target.uri))) }
            is GridLinkTarget.Place -> {
                val to = if (target.sheet == null) sheet else gridSheetIndex(book.names, target.sheet)
                // Excel can't show a hidden sheet either, so a link to one goes nowhere.
                if (to != null && to in tabs) {
                    jump = GridJump(to, target.row, target.col)
                    sheet = to
                }
            }
        }
    }

    Box(Modifier.fillMaxSize()) {
        val loaded by produceState<Result<GridSheet>?>(null, book, sheet) {
            value = null
            value = withContext(Dispatchers.IO) { runCatching { book.sheet(sheet) } }
        }
        when (val result = loaded) {
            null -> Box(Modifier.fillMaxSize(), Alignment.Center) {
                CircularProgressIndicator(Modifier.semantics { contentDescription = "Loading" })
            }
            else -> result.fold(
                { grid ->
                    key(grid) {
                        GridPane(
                            book, grid, zoom, top, bottomBar, scrolls,
                            jump?.takeIf { it.sheet == grid.index }, { jump = null },
                            onTap, onLink,
                            if (thumbSaved) null else ({ bitmap: Bitmap -> thumbSaved = true; onFirstPage(bitmap) }),
                        )
                    }
                },
                {
                    Text(
                        it.message ?: "This sheet can't be shown.",
                        Modifier.align(Alignment.Center).padding(32.dp),
                        textAlign = TextAlign.Center, style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                },
            )
        }

        AnimatedVisibility(
            chrome,
            Modifier.align(Alignment.BottomCenter),
            enter = slideInVertically(tween(220)) { it } + fadeIn(tween(180)),
            exit = slideOutVertically(tween(200)) { it } + fadeOut(tween(150)),
        ) {
            Surface(Modifier.onSizeChanged { bottomBar = it.height }, color = MaterialTheme.colorScheme.surfaceContainer) {
                Row(Modifier.navigationBarsPadding(), verticalAlignment = Alignment.CenterVertically) {
                    SecondaryScrollableTabRow(
                        selectedTabIndex = tabs.indexOf(sheet).coerceAtLeast(0),
                        modifier = Modifier.weight(1f),
                        containerColor = Color.Transparent,
                        edgePadding = 0.dp,
                    ) {
                        for (index in tabs) {
                            Tab(
                                selected = index == sheet,
                                onClick = { sheet = index },
                                text = { Text(book.tabs[index].name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            )
                        }
                    }
                    IconButton({ zoomTo(zoom.floatValue / 1.5f) }, enabled = zoom.floatValue > GRID_MIN_ZOOM) {
                        Icon(painterResource(R.drawable.ic_zoom_out), "Zoom out")
                    }
                    IconButton({ zoomTo(zoom.floatValue * 1.5f) }, enabled = zoom.floatValue < GRID_MAX_ZOOM) {
                        Icon(painterResource(R.drawable.ic_zoom_in), "Zoom in")
                    }
                }
            }
        }
    }
}

@Composable
private fun GridPane(
    book: SheetGridBook,
    grid: GridSheet,
    zoom: MutableFloatState,
    top: Dp,
    bottomBar: Int,
    scrolls: HashMap<Int, GridScroll>,
    jump: GridJump?,
    onJumped: () -> Unit,
    onTap: () -> Unit,
    onLink: (GridLinkTarget) -> Unit,
    onFirstFrame: ((Bitmap) -> Unit)?,
) {
    val density = LocalDensity.current
    val scheme = MaterialTheme.colorScheme
    val scope = rememberCoroutineScope()
    val cells = remember(grid) { GridCells(book, grid) }
    val headerTextPx = with(density) { 12.sp.toPx() }
    val headerH = max(with(density) { 24.dp.toPx() }, headerTextPx * 1.8f)
    val headerW = remember(grid, headerTextPx) {
        val digits = max(3, (grid.rowCount + 1).toString().length)
        android.graphics.Paint().apply { textSize = headerTextPx }.measureText("0".repeat(digits)) + with(density) { 16.dp.toPx() }
    }
    val painter = remember(grid, scheme, headerTextPx) {
        GridPainter(
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
    }
    val start = remember(grid) { scrolls[grid.index] ?: GridScroll(0.0, 0.0) }
    var scrollX by remember(grid) { mutableDoubleStateOf(start.x) }
    var scrollY by remember(grid) { mutableDoubleStateOf(start.y) }
    var area by remember { mutableStateOf(IntSize.Zero) }
    val bottomPad by rememberUpdatedState(bottomBar.toFloat())
    val tap by rememberUpdatedState(onTap)
    val link by rememberUpdatedState(onLink)
    DisposableEffect(grid) { onDispose { scrolls[grid.index] = GridScroll(scrollX, scrollY) } }

    fun viewport(height: Float = area.height.toFloat()) = GridViewport(
        grid.cols, grid.rows, grid.layout.frozenCols.toInt(), grid.layout.frozenRows.toInt(),
        density.density * zoom.floatValue, scrollX, scrollY, area.width.toFloat(), height, headerW, headerH,
    )

    /** Scrolls by screen pixels; false when already at the edge both ways. */
    fun scrollBy(dx: Float, dy: Float): Boolean {
        val v = viewport()
        val x = (scrollX + dx / v.scale).coerceIn(0.0, v.maxScrollX(grid.colCount))
        val y = (scrollY + dy / v.scale).coerceIn(0.0, v.maxScrollY(grid.rowCount, bottomPad))
        val moved = x != scrollX || y != scrollY
        scrollX = x
        scrollY = y
        return moved
    }

    // Scales around [centroid], so the cell under the fingers stays put.
    fun zoomBy(factor: Float, centroid: Offset) {
        val before = viewport()
        val old = zoom.floatValue
        val new = (old * factor).coerceIn(GRID_MIN_ZOOM, GRID_MAX_ZOOM)
        if (new == old) return
        zoom.floatValue = new
        val after = viewport()
        scrollX = gridZoomScroll(scrollX, centroid.x - before.bodyX, centroid.x - after.bodyX, before.scale, after.scale)
        scrollY = gridZoomScroll(scrollY, centroid.y - before.bodyY, centroid.y - after.bodyY, before.scale, after.scale)
        scrollBy(0f, 0f)
    }

    fun animateZoomTo(target: Float, centroid: Offset) = scope.launch {
        animate(zoom.floatValue, target, animationSpec = tween(240, easing = FastOutSlowInEasing)) { value, _ ->
            zoomBy(value / zoom.floatValue, centroid)
        }
    }

    // Zooming from the buttons keeps the top-left cell put; this keeps the scroll in range afterwards.
    LaunchedEffect(grid) { snapshotFlow { zoom.floatValue to area }.collect { scrollBy(0f, 0f) } }

    LaunchedEffect(cells) {
        snapshotFlow { if (area == IntSize.Zero) emptySet() else neededBlocks(viewport(), grid) }
            .distinctUntilChanged()
            .collectLatest { cells.load(it) }
    }

    LaunchedEffect(jump, area) {
        val to = jump ?: return@LaunchedEffect
        if (area == IntSize.Zero) return@LaunchedEffect
        val v = viewport()
        if (to.col >= v.frozenCols) scrollX = grid.cols.start(to.col) - grid.cols.start(v.frozenCols)
        if (to.row >= v.frozenRows) scrollY = grid.rows.start(to.row) - grid.rows.start(v.frozenRows)
        scrollBy(0f, 0f)
        onJumped()
    }

    if (onFirstFrame != null) {
        LaunchedEffect(cells) {
            snapshotFlow { cells.version; area != IntSize.Zero && cells.loaded(neededBlocks(viewport(), grid)) }.first { it }
            // A page-shaped picture of the first screen, for the recent files list.
            val v = viewport(min(area.height.toFloat(), area.width * 1.3f))
            val bitmap = Bitmap.createBitmap(v.width.toInt(), v.height.toInt(), Bitmap.Config.ARGB_8888)
            painter.draw(android.graphics.Canvas(bitmap), v)
            onFirstFrame(bitmap)
        }
    }

    val dispatcher = remember { NestedScrollDispatcher() }
    val connection = remember { object : NestedScrollConnection {} }
    val name = book.tabs.getOrNull(grid.index)?.name.orEmpty()
    Canvas(
        Modifier.fillMaxSize()
            .padding(top = top)
            .onSizeChanged { area = it }
            .nestedScroll(connection, dispatcher)
            .semantics { contentDescription = "Sheet $name, ${grid.rowCount} rows by ${grid.colCount} columns" }
            .pointerInput(grid) {
                val decay = splineBasedDecay<Float>(this)
                coroutineScope {
                    val flings = this
                    var fling: Job? = null
                    awaitEachGesture {
                        val down = awaitFirstDown(requireUnconsumed = false)
                        fling?.cancel()
                        val tracker = VelocityTracker()
                        tracker.addPosition(down.uptimeMillis, down.position)
                        var moved = Offset.Zero
                        var dragging = false
                        var pinched = false
                        // A drag that starts mostly along one axis stays on it, as spreadsheets scroll.
                        var noX = false
                        var noY = false
                        do {
                            val event = awaitPointerEvent()
                            if (event.changes.count { it.pressed } >= 2) {
                                pinched = true
                                val z = event.calculateZoom()
                                if (z != 1f) zoomBy(z, event.calculateCentroid())
                                val pan = event.calculatePan()
                                scrollBy(-pan.x, -pan.y)
                                event.changes.forEach { it.consume() }
                            } else if (!pinched) {
                                val pan = event.calculatePan()
                                if (!dragging) {
                                    moved += pan
                                    if (moved.getDistance() > viewConfiguration.touchSlop) {
                                        dragging = true
                                        noX = abs(moved.y) > 2 * abs(moved.x)
                                        noY = abs(moved.x) > 2 * abs(moved.y)
                                    }
                                }
                                if (dragging) {
                                    val dx = if (noX) 0f else pan.x
                                    val dy = if (noY) 0f else pan.y
                                    scrollBy(-dx, -dy)
                                    // Lets the viewer hide or show its bars, as a scrolling page does.
                                    dispatcher.dispatchPreScroll(Offset(dx, dy), NestedScrollSource.UserInput)
                                    event.changes.forEach { it.consume() }
                                }
                                event.changes.firstOrNull { it.id == down.id }?.let { tracker.addPosition(it.uptimeMillis, it.position) }
                            }
                        } while (event.changes.any { it.pressed })
                        if (dragging && !pinched) {
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
                    }
                }
            }
            .pointerInput(grid) {
                detectTapGestures(
                    onTap = { p ->
                        val target = viewport().let { v -> linkAt(grid, v.rowAt(p.y), v.colAt(p.x)) }
                        if (target != null) link(target) else tap()
                    },
                    onDoubleTap = { p ->
                        val z = zoom.floatValue
                        animateZoomTo(if (z < 1f) 1f else if (z < 2f) 2f else 1f, p)
                    },
                )
            },
    ) {
        cells.version // redraw as blocks arrive
        drawIntoCanvas { painter.draw(it.nativeCanvas, viewport()) }
    }
    if (cells.failed) {
        Text(
            "Some cells couldn't be read. Scroll to try again.",
            Modifier.padding(top = top + with(density) { headerH.toDp() }).fillMaxWidth()
                .background(scheme.surfaceContainerHigh).padding(horizontal = 16.dp, vertical = 8.dp)
                .semantics { liveRegion = LiveRegionMode.Polite },
            style = MaterialTheme.typography.bodyMedium,
            color = scheme.onSurfaceVariant,
        )
    }
}

/** Blocks the screen shows, those just beside it whose text may run in, and the anchors of merges in view. */
private fun neededBlocks(v: GridViewport, grid: GridSheet): Set<GridBlockKey> {
    val frozenRows = 0 until v.frozenRows
    val frozenCols = 0 until v.frozenCols
    val bodyCols = max(v.frozenCols, v.bodyCols.first - GRID_SPILL_COLS)..v.bodyCols.last + GRID_SPILL_COLS
    val rowRanges = listOf(frozenRows, v.bodyRows)
    val keys = LinkedHashSet<GridBlockKey>()
    keys += gridBlocks(listOf(v.bodyRows), listOf(bodyCols))
    keys += gridBlocks(rowRanges, listOf(frozenCols, bodyCols))
    val merges = ArrayList<GridRect>()
    for (rows in rowRanges) for (cols in listOf(frozenCols, v.bodyCols)) grid.merges.inWindow(rows, cols, merges)
    for (m in merges) keys += GridBlockKey(m.firstRow / GRID_BLOCK_ROWS, m.firstCol / GRID_BLOCK_COLS)
    return keys
}

/** Where the link on a cell (or on the merge it sits in) goes, or null. */
private fun linkAt(grid: GridSheet, row: Int?, col: Int?): GridLinkTarget? {
    if (row == null || col == null || grid.links.isEmpty()) return null
    val merge = grid.merges.at(row, col)
    val r = (merge?.firstRow ?: row).toUInt()
    val c = (merge?.firstCol ?: col).toUInt()
    val hit = grid.links.firstOrNull { r in it.range.firstRow..it.range.lastRow && c in it.range.firstCol..it.range.lastCol }
        ?: return null
    return gridLinkTarget(hit.url, hit.location)
}
