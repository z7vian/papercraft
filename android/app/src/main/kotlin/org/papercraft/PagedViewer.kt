package org.papercraft

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
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
import kotlin.math.ceil

/** A file shown as a column of drawn pages: PDF pages, document pages, slides or sheets. */
interface Pages {
    /** Null when only drawing reveals it (Word documents): pages then appear until [render] returns null. */
    val pageCount: Int?

    /** A caption shown above each page (sheet names), or null for none. */
    fun title(index: Int): String? = null

    /** Page [index] about [width] pixels wide, or null past the last page. */
    suspend fun render(index: Int, width: Int): Bitmap?

    /** Frees the file once any render in flight has finished. */
    fun closeLater()
}

@Composable
fun PagedViewer(doc: Pages) {
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val screenWidth = maxWidth
        var zoom by remember { mutableFloatStateOf(1f) }
        val zoomState = rememberTransformableState { _, change, _, _ -> zoom = (zoom * change).coerceIn(1f, 4f) }
        // Re-render only at 1x/2x/3x steps, not on every pinch frame.
        val renderWidth = with(LocalDensity.current) { (screenWidth * ceil(zoom).coerceAtMost(3f)).roundToPx() }.coerceAtMost(2048)
        val list = rememberLazyListState()
        // Page shapes we've seen, so pages don't jump size as they scroll back in.
        val aspects = remember(doc) { mutableStateMapOf<Int, Float>() }
        var lastPage by remember(doc) { mutableStateOf(doc.pageCount) }
        var shown by remember(doc) { mutableIntStateOf(doc.pageCount ?: 1) }

        // Limit: zoom keeps the left edge fixed; zoom around the fingers later.
        Box(
            Modifier.fillMaxSize()
                .pointerInput(Unit) { detectTapGestures(onDoubleTap = { zoom = if (zoom > 1f) 1f else 2f }) }
                .transformable(zoomState, canPan = { false })
                .horizontalScroll(rememberScrollState()),
        ) {
            LazyColumn(
                Modifier.width(screenWidth * zoom).fillMaxHeight(),
                state = list,
                contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 12.dp, bottom = 96.dp),
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
                        }
                        if (bitmap.isSuccess && bitmap.getOrNull() == null && lastPage == null) {
                            lastPage = i
                            shown = i
                        }
                    }
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
                            Modifier.fillMaxWidth().aspectRatio(aspect).shadow(2.dp).background(Color.White)
                                .semantics { contentDescription = "Page ${i + 1}" + (lastPage?.let { " of $it" } ?: "") },
                        ) {
                            page?.let { Image(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.FillBounds) }
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
        val total = lastPage?.toString() ?: "…"
        Surface(
            Modifier.align(Alignment.BottomCenter).padding(16.dp),
            shape = CircleShape,
            color = MaterialTheme.colorScheme.surfaceContainerHigh,
            shadowElevation = 6.dp,
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconButton({ zoom = (zoom / 1.5f).coerceAtLeast(1f) }, enabled = zoom > 1f) {
                    Icon(painterResource(R.drawable.ic_zoom_out), "Zoom out")
                }
                Text(
                    "$current / $total",
                    Modifier.padding(horizontal = 4.dp).clearAndSetSemantics {
                        contentDescription = "Page $current" + (lastPage?.let { " of $it" } ?: "")
                    },
                    style = MaterialTheme.typography.labelLarge,
                )
                IconButton({ zoom = (zoom * 1.5f).coerceAtMost(4f) }, enabled = zoom < 4f) {
                    Icon(painterResource(R.drawable.ic_zoom_in), "Zoom in")
                }
            }
        }
    }
}
