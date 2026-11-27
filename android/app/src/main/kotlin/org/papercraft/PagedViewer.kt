package org.papercraft

import android.graphics.Bitmap
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import kotlin.math.ceil

/** A file shown as a column of drawn pages: PDF pages, document pages, slides or sheets. */
interface Pages {
    /** Null when only drawing reveals it (Word documents): pages then appear until [render] returns null. */
    val pageCount: Int?

    /** A caption shown above each page (sheet names), or null for none. */
    fun title(index: Int): String? = null

    /** Page [index] about [width] pixels wide, or null past the last page. */
    suspend fun render(index: Int, width: Int): Bitmap?

    /** Matches of [query] in page order, at most [limit]; stops between pages once its coroutine is cancelled. */
    suspend fun find(query: String, limit: Int): List<PageHit>

    /** Frees the file once any render in flight has finished. */
    fun closeLater()
}

private const val MAX_ZOOM = 4f

/**
 * The page column. It fills the screen under the bars ([padding] keeps the first page clear of them),
 * so showing or hiding the bars never moves a page.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PagedViewer(
    doc: Pages,
    padding: PaddingValues,
    chrome: Boolean,
    onTap: () -> Unit,
    onStatus: (String) -> Unit,
    onFirstPage: (Bitmap) -> Unit,
    find: FindState,
) {
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val screenWidth = maxWidth
        val scope = rememberCoroutineScope()
        var zoom by remember { mutableFloatStateOf(1f) }
        val hScroll = rememberScrollState()
        val list = rememberLazyListState()
        // Re-render only at 1x/2x/3x steps, not on every pinch frame; the last render stays on screen meanwhile.
        val renderWidth = with(LocalDensity.current) { (screenWidth * ceil(zoom).coerceAtMost(3f)).roundToPx() }.coerceAtMost(2048)
        // Page shapes we've seen, so pages don't jump size as they scroll back in.
        val aspects = remember(doc) { mutableStateMapOf<Int, Float>() }
        var lastPage by remember(doc) { mutableStateOf(doc.pageCount) }
        var shown by remember(doc) { mutableIntStateOf(doc.pageCount ?: 1) }
        var thumbSaved by remember(doc) { mutableStateOf(false) }
        var pagesSheet by remember { mutableStateOf(false) }
        val density = LocalDensity.current
        val hits = rememberHits<PageHit>(
            find, doc,
            startAt = { found -> found.firstAtOrAfter(list.firstVisibleItemIndex) { it.page } },
            pagesOnly = { found -> found.all { it.boxes.isEmpty() } },
        ) { query -> doc.find(query, MAX_FIND_HITS) }
        val hitsOnPage = remember(hits) { hits.indices.groupBy { hits[it].page } }
        LaunchedEffect(hits, find.moves) {
            val hit = hits.getOrNull(find.current) ?: return@LaunchedEffect
            if (lastPage == null) shown = maxOf(shown, hit.page + 1)
            val box = hit.boxes.firstOrNull() ?: return@LaunchedEffect list.animateScrollToItem(hit.page)
            // The match lands a third of the way down the screen and, when zoomed in, across into view.
            val pageWidth = with(density) { (screenWidth * zoom - 24.dp).toPx() }
            val px = box.scaled(pageWidth, pageWidth / (aspects[hit.page] ?: aspects[0] ?: (1 / 1.414f)))
            list.animateScrollToItem(hit.page, (px.top - list.layoutInfo.viewportSize.height / 3).toInt().coerceAtLeast(0))
            val left = px.left + with(density) { 12.dp.toPx() } - hScroll.viewportSize / 3
            hScroll.animateScrollTo(left.toInt().coerceIn(0, hScroll.maxValue))
        }

        // Scales around [centroid], so the point under the fingers stays put.
        fun zoomBy(factor: Float, centroid: Offset) {
            val old = zoom
            val new = (old * factor).coerceIn(1f, MAX_ZOOM)
            if (new == old) return
            zoom = new
            val f = new / old
            val dx = (hScroll.value + centroid.x) * (f - 1)
            val dy = (list.firstVisibleItemScrollOffset + centroid.y) * (f - 1)
            // The content is only wider and taller after the next layout, so scroll once it exists.
            scope.launch {
                withFrameNanos { }
                hScroll.scrollBy(dx)
                list.scrollBy(dy)
            }
        }

        fun animateZoomTo(target: Float, centroid: Offset) = scope.launch {
            animate(zoom, target, animationSpec = tween(240, easing = FastOutSlowInEasing)) { value, _ -> zoomBy(value / zoom, centroid) }
        }
        val center = with(LocalDensity.current) { Offset(screenWidth.toPx() / 2, maxHeight.toPx() / 2) }

        Box(
            Modifier.fillMaxSize()
                // Two fingers zoom; the Initial pass takes them before the page list can scroll with them.
                .pointerInput(Unit) {
                    awaitEachGesture {
                        awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                        do {
                            val event = awaitPointerEvent(PointerEventPass.Initial)
                            if (event.changes.count { it.pressed } >= 2) {
                                val z = event.calculateZoom()
                                if (z != 1f) zoomBy(z, event.calculateCentroid())
                                event.changes.forEach { it.consume() }
                            }
                        } while (event.changes.any { it.pressed })
                    }
                }
                .pointerInput(Unit) {
                    detectTapGestures(onTap = { onTap() }, onDoubleTap = { p -> animateZoomTo(if (zoom > 1f) 1f else 2f, p) })
                }
                .horizontalScroll(hScroll),
        ) {
            LazyColumn(
                Modifier.width(screenWidth * zoom).fillMaxHeight(),
                state = list,
                contentPadding = PaddingValues(
                    start = 12.dp, end = 12.dp,
                    top = padding.calculateTopPadding() + 12.dp,
                    bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 96.dp,
                ),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                items(shown) { i ->
                    var page by remember { mutableStateOf<ImageBitmap?>(null) }
                    LaunchedEffect(renderWidth) {
                        // A broken page shows blank instead of crashing the app.
                        val bitmap = runCatching { doc.render(i, renderWidth) }
                        bitmap.getOrNull()?.let {
                            page = it.asImageBitmap()
                            aspects[i] = it.width.toFloat() / it.height
                            if (lastPage == null) shown = maxOf(shown, i + 2)
                            if (i == 0 && !thumbSaved) { thumbSaved = true; onFirstPage(it) }
                        }
                        if (bitmap.isSuccess && bitmap.getOrNull() == null && lastPage == null) {
                            lastPage = i
                            shown = i
                        }
                    }
                    val alpha by animateFloatAsState(if (page != null) 1f else 0f, tween(220), label = "page")
                    val aspect = aspects[i] ?: aspects[0] ?: (1 / 1.414f) // A4 until we know better
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        doc.title(i)?.let {
                            Text(
                                it, Modifier.semantics { heading() },
                                style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 1, overflow = TextOverflow.Ellipsis,
                            )
                        }
                        Box(
                            Modifier.fillMaxWidth().aspectRatio(aspect).shadow(1.dp, MaterialTheme.shapes.extraSmall)
                                .background(Color.White)
                                .semantics { contentDescription = "Page ${i + 1}" + (lastPage?.let { " of $it" } ?: "") },
                        ) {
                            page?.let { Image(it, null, Modifier.fillMaxSize().graphicsLayer { this.alpha = alpha }, contentScale = ContentScale.FillBounds) }
                            hitsOnPage[i]?.let { PageHighlights(hits, it, find.current) }
                        }
                    }
                }
            }
        }

        // Once scrolled to the end, the last page counts as current even if a taller one sits mid-screen.
        val current by remember {
            derivedStateOf {
                val info = list.layoutInfo
                val middle = (info.viewportStartOffset + info.viewportEndOffset) / 2
                if (!list.canScrollForward && list.canScrollBackward) shown
                else (info.visibleItemsInfo.lastOrNull { it.offset <= middle }?.index ?: 0) + 1
            }
        }
        LaunchedEffect(current, lastPage) { onStatus("Page $current of ${lastPage ?: "…"}") }

        AnimatedVisibility(
            chrome,
            Modifier.align(Alignment.BottomCenter).padding(WindowInsets.navigationBars.asPaddingValues()).padding(16.dp),
            enter = slideInVertically(tween(220)) { it } + fadeIn(tween(180)),
            exit = slideOutVertically(tween(200)) { it } + fadeOut(tween(150)),
        ) {
            Surface(shape = CircleShape, color = MaterialTheme.colorScheme.surfaceContainerHigh, shadowElevation = 6.dp) {
                Row(Modifier.padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconButton({ pagesSheet = true }) { Icon(painterResource(R.drawable.ic_pages), "All pages") }
                    IconButton({ animateZoomTo((zoom / 1.5f).coerceAtLeast(1f), center) }, enabled = zoom > 1f) {
                        Icon(painterResource(R.drawable.ic_zoom_out), "Zoom out")
                    }
                    Text(
                        "$current / ${lastPage ?: "…"}",
                        Modifier.padding(horizontal = 4.dp).clearAndSetSemantics {
                            contentDescription = "Page $current" + (lastPage?.let { " of $it" } ?: "")
                        },
                        style = MaterialTheme.typography.labelLarge,
                    )
                    IconButton({ animateZoomTo((zoom * 1.5f).coerceAtMost(MAX_ZOOM), center) }, enabled = zoom < MAX_ZOOM) {
                        Icon(painterResource(R.drawable.ic_zoom_in), "Zoom in")
                    }
                    IconButton({ animateZoomTo(1f, center) }, enabled = zoom > 1f) { Icon(painterResource(R.drawable.ic_fit), "Fit to width") }
                }
            }
        }

        if (pagesSheet) {
            ModalBottomSheet({ pagesSheet = false }) {
                LazyVerticalGrid(
                    GridCells.Adaptive(96.dp),
                    contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 24.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    items(shown) { i ->
                        var thumb by remember { mutableStateOf<ImageBitmap?>(null) }
                        LaunchedEffect(Unit) { thumb = runCatching { doc.render(i, 240) }.getOrNull()?.asImageBitmap() }
                        val alpha by animateFloatAsState(if (thumb != null) 1f else 0f, tween(180), label = "thumb")
                        Column(
                            Modifier.clip(MaterialTheme.shapes.small).clickable(onClickLabel = "Go to page ${i + 1}") {
                                pagesSheet = false
                                scope.launch { list.animateScrollToItem(i) }
                            },
                            horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(4.dp),
                        ) {
                            Box(
                                Modifier.fillMaxWidth().aspectRatio(aspects[i] ?: aspects[0] ?: (1 / 1.414f))
                                    .shadow(1.dp, MaterialTheme.shapes.extraSmall).background(Color.White),
                            ) {
                                thumb?.let { Image(it, null, Modifier.fillMaxSize().graphicsLayer { this.alpha = alpha }, contentScale = ContentScale.FillBounds) }
                            }
                            Text(
                                "${i + 1}", style = MaterialTheme.typography.labelMedium,
                                color = if (i + 1 == current) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
        }
    }
}
