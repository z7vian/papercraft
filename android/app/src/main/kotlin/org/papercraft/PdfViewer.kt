package org.papercraft

import android.content.Context
import android.graphics.Bitmap
import android.graphics.pdf.PdfRenderer
import android.net.Uri
import android.os.ParcelFileDescriptor
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
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
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.FileNotFoundException
import kotlin.math.ceil
import kotlin.math.sqrt

class PasswordProtectedException : Exception("This PDF is password-protected. Password support is coming later.")

/** Android's built-in PDF renderer. It's not thread-safe, so every call goes through one lock. */
class PdfDoc private constructor(fd: ParcelFileDescriptor) {
    private val renderer = PdfRenderer(fd) // takes ownership of fd
    private val lock = Mutex()
    private var closed = false
    val pageCount = renderer.pageCount

    suspend fun render(index: Int, width: Int): Bitmap? = lock.withLock {
        if (closed) return null
        withContext(Dispatchers.IO) {
            renderer.openPage(index).use { page ->
                // Limit: max 16 MP (64 MB) per page, under Android's 100 MB draw limit; tile very tall pages to make them sharper.
                val w = minOf(width.toDouble(), sqrt(16e6 * page.width / page.height)).toInt().coerceAtLeast(1)
                val h = (w.toLong() * page.height / page.width).toInt().coerceAtLeast(1)
                Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888).also {
                    it.eraseColor(android.graphics.Color.WHITE)
                    page.render(it, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                }
            }
        }
    }

    /** Closes once any page render in flight has finished. */
    fun closeLater() {
        CoroutineScope(Dispatchers.IO).launch { lock.withLock { closed = true; renderer.close() } }
    }

    companion object {
        /** Blocking. PdfRenderer throws SecurityException for a password-protected file. */
        fun open(context: Context, uri: Uri): PdfDoc {
            val fd = context.contentResolver.openFileDescriptor(uri, "r") ?: throw FileNotFoundException()
            return try {
                PdfDoc(fd)
            } catch (e: SecurityException) {
                fd.close()
                throw PasswordProtectedException()
            } catch (t: Throwable) {
                fd.close()
                throw t
            }
        }
    }
}

@Composable
fun PdfViewer(doc: PdfDoc) {
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val screenWidth = maxWidth
        var zoom by remember { mutableFloatStateOf(1f) }
        val zoomState = rememberTransformableState { _, change, _, _ -> zoom = (zoom * change).coerceIn(1f, 4f) }
        // Re-render only at 1x/2x/3x steps, not on every pinch frame.
        val renderWidth = with(LocalDensity.current) { (screenWidth * ceil(zoom).coerceAtMost(3f)).roundToPx() }.coerceAtMost(2048)
        val list = rememberLazyListState()
        // Page shapes we've seen, so pages don't jump size as they scroll back in.
        val aspects = remember(doc) { mutableStateMapOf<Int, Float>() }

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
                items(doc.pageCount) { i ->
                    var page by remember { mutableStateOf<ImageBitmap?>(null) }
                    LaunchedEffect(renderWidth) {
                        // A broken page shows blank instead of crashing the app.
                        runCatching { doc.render(i, renderWidth) }.getOrNull()?.let {
                            page = it.asImageBitmap()
                            aspects[i] = it.width.toFloat() / it.height
                        }
                    }
                    val aspect = aspects[i] ?: aspects[0] ?: (1 / 1.414f) // A4 until we know better
                    Box(
                        Modifier.fillMaxWidth().aspectRatio(aspect).shadow(2.dp).background(Color.White)
                            .semantics { contentDescription = "Page ${i + 1} of ${doc.pageCount}" },
                    ) {
                        page?.let { Image(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.FillBounds) }
                    }
                }
            }
        }

        // Once scrolled to the end, the last page counts as current even if a taller one sits mid-screen.
        val current by remember {
            derivedStateOf {
                val info = list.layoutInfo
                val middle = (info.viewportStartOffset + info.viewportEndOffset) / 2
                if (!list.canScrollForward && list.canScrollBackward) doc.pageCount
                else (info.visibleItemsInfo.lastOrNull { it.offset <= middle }?.index ?: 0) + 1
            }
        }
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
                    "$current / ${doc.pageCount}",
                    Modifier.padding(horizontal = 4.dp).clearAndSetSemantics { contentDescription = "Page $current of ${doc.pageCount}" },
                    style = MaterialTheme.typography.labelLarge,
                )
                IconButton({ zoom = (zoom * 1.5f).coerceAtMost(4f) }, enabled = zoom < 4f) {
                    Icon(painterResource(R.drawable.ic_zoom_in), "Zoom in")
                }
            }
        }
    }
}
