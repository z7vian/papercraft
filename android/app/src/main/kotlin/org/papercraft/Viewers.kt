package org.papercraft

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.graphics.pdf.PdfRenderer
import android.media.ExifInterface
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
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
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.papercraft.core.WebArchive
import org.papercraft.core.WebResource
import org.papercraft.core.decodeText
import org.papercraft.core.decodeTiff
import org.papercraft.core.parseCsv
import org.papercraft.core.parseMhtml
import java.io.ByteArrayInputStream
import java.io.FileNotFoundException
import java.nio.ByteBuffer
import kotlin.math.ceil
import kotlin.math.sqrt

// Limit: text/CSV become ~5x their size in memory; stream them to lift this.
private const val MAX_TEXT_BYTES = 30_000_000L

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ViewerScreen(file: DocFile, onBack: () -> Unit) {
    val context = LocalContext.current
    val viewer = viewerOf(file.ext)
    var menu by remember { mutableStateOf(false) }
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    BackHandler(onBack = onBack)

    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection).semantics { paneTitle = file.name },
        topBar = {
            TopAppBar(
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        FileBadge(file.ext, size = 28.dp)
                        Text(file.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                },
                navigationIcon = { IconButton(onBack) { Icon(painterResource(R.drawable.ic_back), "Back") } },
                actions = {
                    IconButton({ share(context, file.uri) }) { Icon(painterResource(R.drawable.ic_share), "Share") }
                    Box {
                        IconButton({ menu = true }) { Icon(painterResource(R.drawable.ic_more), "More options") }
                        DropdownMenu(menu, { menu = false }) {
                            DropdownMenuItem(text = { Text("Open with another app") }, onClick = { menu = false; openElsewhere(context, file.uri) })
                        }
                    }
                },
                scrollBehavior = scroll,
            )
        },
    ) { padding ->
        // Paint the backdrop before padding so it runs under the gesture bar, then clip zoomed content.
        val backdrop = if (viewer == Viewer.Pdf || viewer == Viewer.Image) MaterialTheme.colorScheme.surfaceContainerHighest else Color.Transparent
        Box(Modifier.fillMaxSize().background(backdrop).padding(padding).clipToBounds()) {
            when (viewer) {
                Viewer.Text -> Loaded(file, MAX_TEXT_BYTES, { bytes ->
                    // Split giant lines (minified JSON, logs) so no single Text has to lay out megabytes.
                    decodeText(bytes).lines().flatMap { it.chunked(4000).ifEmpty { listOf("") } }
                }) { TextViewer(it) }
                Viewer.Table -> Loaded(file, MAX_TEXT_BYTES, ::parseCsv) { TableViewer(it) }
                Viewer.Image -> Loaded(file, parse = { decodeImage(it, file.ext) }) { ImageViewer(it, file.name) }
                Viewer.Web -> Loaded(file, parse = { WebPage(decodeText(it), null, emptyMap()) }) { WebViewer(it) }
                Viewer.WebArchive -> Loaded(file, parse = { parseMhtml(it).toWebPage() }) { WebViewer(it) }
                Viewer.Pdf -> PdfViewer(file)
                Viewer.None -> Message(
                    if (categoryOf(file.ext) != null) "Papercraft can't open .${file.ext} files yet. Support is coming in a later version."
                    else "Papercraft can't open ${if (file.ext.isEmpty()) "this kind of" else ".${file.ext}"} files.",
                ) { Button({ openElsewhere(context, file.uri) }) { Text("Open with another app") } }
            }
        }
    }
}

private fun share(context: Context, uri: Uri) {
    val send = Intent(Intent.ACTION_SEND)
        .setType(context.contentResolver.getType(uri) ?: "*/*")
        .putExtra(Intent.EXTRA_STREAM, uri)
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    runCatching { context.startActivity(Intent.createChooser(send, null)) }
}

private fun openElsewhere(context: Context, uri: Uri) {
    val view = Intent(Intent.ACTION_VIEW)
        .setDataAndType(uri, context.contentResolver.getType(uri))
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    val chooser = Intent.createChooser(view, null)
        .putExtra(Intent.EXTRA_EXCLUDE_COMPONENTS, arrayOf(ComponentName(context, MainActivity::class.java)))
    runCatching { context.startActivity(chooser) }
}

/** Reads the file off the main thread, shows a spinner, then [content] or a readable error. */
@Composable
private fun <T> Loaded(file: DocFile, maxBytes: Long = Long.MAX_VALUE, parse: (ByteArray) -> T, content: @Composable (T) -> Unit) {
    val context = LocalContext.current
    val result by produceState<Result<T>?>(null, file.uri) {
        value = withContext(Dispatchers.IO) {
            runCatching {
                if (file.size > maxBytes) throw TooBigException()
                val bytes = context.readBytes(file.uri)
                if (bytes.size > maxBytes) throw TooBigException()
                parse(bytes)
            }
        }
    }
    val r = result ?: return Spinner()
    val value = r.getOrElse { err ->
        return Message(friendlyError(err)) { Button({ openElsewhere(context, file.uri) }) { Text("Open with another app") } }
    }
    content(value)
}

@Composable
private fun Spinner() = Box(Modifier.fillMaxSize(), Alignment.Center) {
    CircularProgressIndicator(Modifier.semantics { contentDescription = "Loading" })
}

@Composable
private fun Message(text: String, action: (@Composable () -> Unit)? = null) = Column(
    Modifier.fillMaxSize().padding(32.dp).semantics { liveRegion = LiveRegionMode.Polite },
    verticalArrangement = Arrangement.Center,
    horizontalAlignment = Alignment.CenterHorizontally,
) {
    Text(text, textAlign = TextAlign.Center, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
    if (action != null) {
        Spacer(Modifier.height(20.dp))
        action()
    }
}

@Composable
private fun TextViewer(lines: List<String>) {
    SelectionContainer {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(horizontal = 20.dp, vertical = 16.dp)) {
            items(lines) { Text(it, style = MaterialTheme.typography.bodyLarge) }
        }
    }
}

@Composable
private fun TableViewer(rows: List<List<String>>) {
    val density = LocalDensity.current
    // Sized in sp so rows and columns grow with the user's font size.
    val rowH = with(density) { 32.sp.toDp() }
    val numW = with(density) { 48.sp.toDp() }
    val cellW = with(density) { 112.sp.toDp() }
    // Limit: Compose can't lay out wider than ~262k px, so columns past that are cut; virtualize columns to lift it.
    val maxCols = (250_000 / with(density) { cellW.toPx() }).toInt()
    val cols = minOf(rows.maxOfOrNull { it.size } ?: 0, maxCols)
    val line = MaterialTheme.colorScheme.outlineVariant
    val headerBg = MaterialTheme.colorScheme.surfaceContainerHigh

    @Composable
    fun Cell(text: String, width: Dp, header: Boolean = false) {
        val numeric = !header && text.trim().toDoubleOrNull() != null
        Box(
            Modifier.width(width).height(rowH)
                .background(if (header) headerBg else Color.Transparent)
                .drawBehind {
                    drawLine(line, Offset(size.width, 0f), Offset(size.width, size.height))
                    drawLine(line, Offset(0f, size.height), Offset(size.width, size.height))
                }
                .padding(horizontal = 8.dp),
            contentAlignment = when {
                header -> Alignment.Center
                numeric -> Alignment.CenterEnd
                else -> Alignment.CenterStart
            },
        ) {
            Text(
                text, maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 13.sp,
                fontWeight = if (header) FontWeight.Medium else FontWeight.Normal,
                color = if (header) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface,
            )
        }
    }

    Box(Modifier.fillMaxSize().horizontalScroll(rememberScrollState())) {
        LazyColumn(Modifier.width(numW + cellW * cols).fillMaxHeight()) {
            stickyHeader {
                Row(Modifier.background(MaterialTheme.colorScheme.surface).clearAndSetSemantics {}) {
                    Cell("", numW, header = true)
                    for (c in 0 until cols) Cell(columnName(c), cellW, header = true)
                }
            }
            itemsIndexed(rows) { i, row ->
                // Screen readers hear one row at a time, with blanks spoken so columns stay aligned.
                Row(Modifier.clearAndSetSemantics {
                    contentDescription = "Row ${i + 1}: " + (0 until cols).joinToString(", ") { row.getOrElse(it) { "" }.ifBlank { "blank" } }
                }) {
                    Cell("${i + 1}", numW, header = true)
                    for (c in 0 until cols) Cell(row.getOrElse(c) { "" }, cellW)
                }
            }
        }
    }
}

/** 0 -> A, 25 -> Z, 26 -> AA. */
private fun columnName(index: Int): String {
    var n = index + 1
    val sb = StringBuilder()
    while (n > 0) {
        n--
        sb.append('A' + n % 26)
        n /= 26
    }
    return sb.reverse().toString()
}

private const val MAX_IMAGE_SIDE = 4096 // bigger bitmaps crash the Canvas (Rust caps TIFF the same way)

private fun decodeImage(bytes: ByteArray, ext: String): ImageBitmap {
    if (ext == "tif" || ext == "tiff") { // Android can't decode TIFF; Rust does it
        val img = decodeTiff(bytes)
        return Bitmap.createBitmap(img.width.toInt(), img.height.toInt(), Bitmap.Config.ARGB_8888)
            .apply { copyPixelsFromBuffer(ByteBuffer.wrap(img.pixels)) }
            .asImageBitmap()
    }
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / sample > MAX_IMAGE_SIDE) sample *= 2
    val bmp = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
        ?: error("This picture looks damaged.")
    // Phone photos are stored sideways with an EXIF "rotate me" tag.
    val degrees = when (ExifInterface(ByteArrayInputStream(bytes)).getAttributeInt(ExifInterface.TAG_ORIENTATION, 0)) {
        ExifInterface.ORIENTATION_ROTATE_90 -> 90f
        ExifInterface.ORIENTATION_ROTATE_180 -> 180f
        ExifInterface.ORIENTATION_ROTATE_270 -> 270f
        else -> 0f
    }
    val upright = if (degrees == 0f) bmp
    else Bitmap.createBitmap(bmp, 0, 0, bmp.width, bmp.height, Matrix().apply { postRotate(degrees) }, true)
    return upright.asImageBitmap()
}

@Composable
private fun ImageViewer(image: ImageBitmap, name: String) {
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    var box by remember { mutableStateOf(IntSize.Zero) }

    fun zoomTo(s: Float, pan: Offset = Offset.Zero) {
        scale = s.coerceIn(1f, 8f)
        // Keep the picture on screen: it can move at most as far as it overflows.
        val mx = box.width * (scale - 1) / 2
        val my = box.height * (scale - 1) / 2
        offset = (offset + pan).let { Offset(it.x.coerceIn(-mx, mx), it.y.coerceIn(-my, my)) }
    }
    val toggle = { zoomTo(if (scale > 1f) 1f else 2.5f) }
    val zoom = rememberTransformableState { _, z, pan, _ -> zoomTo(scale * z, pan) }

    Image(
        image, name,
        Modifier.fillMaxSize()
            .onSizeChanged { box = it }
            .semantics { onClick(label = if (scale > 1f) "Zoom out" else "Zoom in") { toggle(); true } }
            .pointerInput(Unit) { detectTapGestures(onDoubleTap = { toggle() }) }
            .transformable(zoom)
            .graphicsLayer { scaleX = scale; scaleY = scale; translationX = offset.x; translationY = offset.y },
        contentScale = ContentScale.Fit,
    )
}

private class WebPage(val html: String, val baseUrl: String?, val resources: Map<String, WebResource>)

private fun WebArchive.toWebPage() = WebPage(html, baseUrl, resources.flatMap { r -> r.urls.map { it to r } }.toMap())

@Composable
private fun WebViewer(page: WebPage) {
    val context = LocalContext.current
    AndroidView(
        modifier = Modifier.fillMaxSize(),
        onRelease = { it.destroy() },
        factory = { ctx ->
            WebView(ctx).apply {
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                settings.builtInZoomControls = true
                settings.displayZoomControls = false
                webViewClient = object : WebViewClient() {
                    // Serve the archive's own files; block the internet so opening a page never phones home.
                    // The page itself arrives as data:, so only http(s) is blocked.
                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                        val r = page.resources[request.url.toString()]
                        return when {
                            r != null -> WebResourceResponse(r.mime, null, ByteArrayInputStream(r.data))
                            request.url.scheme in setOf("http", "https") ->
                                WebResourceResponse("text/plain", "utf-8", 403, "Blocked", emptyMap(), ByteArrayInputStream(ByteArray(0)))
                            else -> null
                        }
                    }

                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                        runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, request.url)) }
                        return true
                    }
                }
                loadDataWithBaseURL(page.baseUrl, page.html, "text/html", "utf-8", null)
            }
        },
    )
}

/** Android's built-in PDF renderer. It's not thread-safe, so every call goes through one lock. */
private class PdfDoc(fd: ParcelFileDescriptor) {
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

    suspend fun close() = lock.withLock { closed = true; renderer.close() }
}

@Composable
private fun PdfViewer(file: DocFile) {
    val context = LocalContext.current
    val opened by produceState<Result<PdfDoc>?>(null, file.uri) {
        // NonCancellable: if the user backs out mid-open we still get the doc, so awaitDispose can close it.
        val doc = withContext(NonCancellable + Dispatchers.IO) {
            runCatching {
                val fd = try {
                    context.contentResolver.openFileDescriptor(file.uri, "r")
                } catch (e: SecurityException) {
                    throw FileNotFoundException(e.message) // lost access, not a password
                } ?: throw FileNotFoundException()
                try { PdfDoc(fd) } catch (t: Throwable) { fd.close(); throw t }
            }
        }
        value = doc
        awaitDispose { doc.getOrNull()?.let { CoroutineScope(Dispatchers.IO).launch { it.close() } } }
    }
    val result = opened ?: return Spinner()
    val doc = result.getOrElse {
        return Message(
            if (it is SecurityException) "This PDF is password-protected. Password support is coming later."
            else friendlyError(it),
        )
    }

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

        // The page under the middle of the screen; the last page once scrolled to the end.
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
