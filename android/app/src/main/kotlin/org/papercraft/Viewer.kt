package org.papercraft

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.graphics.Bitmap
import androidx.activity.compose.BackHandler
import androidx.activity.compose.LocalActivity
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsIgnoringVisibility
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.TopAppBarScrollBehavior
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.FileNotFoundException

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ViewerScreen(file: DocFile, onBack: () -> Unit) {
    val context = LocalContext.current
    val viewer = viewerOf(file.ext)
    val editor = editorOf(file.ext)
    var editing by rememberSaveable { mutableStateOf(false) }
    if (editing && editor != null) return editor.screen(file, false) { editing = false }
    var status by remember { mutableStateOf<String?>(null) }
    var chrome by rememberSaveable { mutableStateOf(true) }
    val scope = rememberCoroutineScope()
    val saveThumb: (Bitmap) -> Unit = { bmp -> scope.launch(Dispatchers.IO) { runCatching { Thumbnails.save(context, file.uri, bmp) } } }
    BackHandler(onBack = onBack)

    if (viewer?.fullScreen == true) {
        // The bars float over the content, so hiding them never moves it.
        SystemBarsVisible(chrome)
        val hideOnScroll = remember {
            object : NestedScrollConnection {
                override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                    if (available.y < -6f) chrome = false else if (available.y > 6f) chrome = true
                    return Offset.Zero
                }
            }
        }
        val barSpace = PaddingValues(top = WindowInsets.statusBarsIgnoringVisibility.asPaddingValues().calculateTopPadding() + 64.dp)
        Box(
            Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerHighest)
                .nestedScroll(hideOnScroll).clipToBounds().semantics { paneTitle = file.name },
        ) {
            val toggle = { chrome = !chrome }
            when (viewer) {
                Viewer.Image -> Loaded(file, { loadImage(file) }) {
                    LaunchedEffect(it) { saveThumb(it.asAndroidBitmap()) }
                    ImageViewer(it, file.name, toggle)
                }
                Viewer.Tiff -> Loaded(file, { loadTiff(file) }) {
                    LaunchedEffect(it) { saveThumb(it.asAndroidBitmap()) }
                    ImageViewer(it, file.name, toggle)
                }
                Viewer.Office -> Loaded<Pages>(file, { OfficePages.open(this, file) }, release = Pages::closeLater) {
                    PagedViewer(it, barSpace, chrome, toggle, { s -> status = s }, saveThumb)
                }
                Viewer.Pdf -> Loaded<Pages>(file, { PdfDoc.open(this, file.uri) }, release = Pages::closeLater) {
                    PagedViewer(it, barSpace, chrome, toggle, { s -> status = s }, saveThumb)
                }
                else -> error("not a full-screen viewer")
            }
            AnimatedVisibility(
                chrome,
                enter = slideInVertically(tween(220)) { -it } + fadeIn(tween(180)),
                exit = slideOutVertically(tween(200)) { -it } + fadeOut(tween(150)),
            ) {
                ViewerTopBar(file, status, onBack, scroll = null, windowInsets = WindowInsets.statusBarsIgnoringVisibility, onEdit = editor?.let { { editing = true } })
            }
        }
    } else {
        val scroll = TopAppBarDefaults.enterAlwaysScrollBehavior()
        Scaffold(
            modifier = Modifier.nestedScroll(scroll.nestedScrollConnection).semantics { paneTitle = file.name },
            topBar = { ViewerTopBar(file, status, onBack, scroll, TopAppBarDefaults.windowInsets) },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding).clipToBounds()) {
                when (viewer) {
                    Viewer.Text -> Loaded(file, { loadText(file) }) { TextViewer(it, monospace = file.ext == "prn") }
                    Viewer.Table -> Loaded(file, { loadTable(file) }) { TableViewer(it) }
                    Viewer.Web -> Loaded(file, { loadWebPage(file) }) { WebViewer(it) }
                    Viewer.WebArchive -> Loaded(file, { loadWebArchive(file) }) { WebViewer(it) }
                    null -> CantOpen(
                        file,
                        if (categoryOf(file.ext) != null) "Papercraft can't open .${file.ext} files yet. Support is coming in a later version."
                        else "Papercraft can't open ${if (file.ext.isEmpty()) "this kind of" else ".${file.ext}"} files.",
                    )
                    else -> error("full-screen viewer in the bar layout")
                }
            }
        }
    }
}

/** Read mode hides the status and navigation bars with the app's own; a swipe from the edge shows them briefly. */
@Composable
private fun SystemBarsVisible(visible: Boolean) {
    val window = LocalActivity.current?.window ?: return
    DisposableEffect(visible) {
        val bars = WindowCompat.getInsetsController(window, window.decorView)
        bars.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        if (visible) bars.show(WindowInsetsCompat.Type.systemBars()) else bars.hide(WindowInsetsCompat.Type.systemBars())
        onDispose { bars.show(WindowInsetsCompat.Type.systemBars()) }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ViewerTopBar(file: DocFile, status: String?, onBack: () -> Unit, scroll: TopAppBarScrollBehavior?, windowInsets: WindowInsets, onEdit: (() -> Unit)? = null) {
    val context = LocalContext.current
    var menu by remember { mutableStateOf(false) }
    TopAppBar(
        title = {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                FileBadge(file.ext, size = 28.dp)
                Column {
                    Text(file.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    status?.let { Text(it, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1) }
                }
            }
        },
        navigationIcon = { IconButton(onBack) { Icon(painterResource(R.drawable.ic_back), "Back") } },
        actions = {
            onEdit?.let { IconButton(it) { Icon(painterResource(R.drawable.ic_edit), "Edit") } }
            IconButton({ share(context, file.uri) }) { Icon(painterResource(R.drawable.ic_share), "Share") }
            Box {
                IconButton({ menu = true }) { Icon(painterResource(R.drawable.ic_more), "More options") }
                DropdownMenu(menu, { menu = false }) {
                    DropdownMenuItem(text = { Text("Open with another app") }, onClick = { menu = false; openElsewhere(context, file.uri) })
                }
            }
        },
        windowInsets = windowInsets,
        colors = TopAppBarDefaults.topAppBarColors(
            containerColor = MaterialTheme.colorScheme.surfaceContainer,
            scrolledContainerColor = MaterialTheme.colorScheme.surfaceContainer,
        ),
        scrollBehavior = scroll,
    )
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

/** [release] closes what [load] returned; loading finishes even if the user leaves, so nothing opened is lost. */
@Composable
private fun <T : Any> Loaded(file: DocFile, load: Context.() -> T, release: (T) -> Unit = {}, content: @Composable (T) -> Unit) {
    val context = LocalContext.current
    val result by produceState<Result<T>?>(null, file.uri) {
        val loaded = withContext(NonCancellable + Dispatchers.IO) { runCatching { context.load() } }
        value = loaded
        awaitDispose { loaded.getOrNull()?.let(release) }
    }
    val r = result ?: return Spinner()
    content(r.getOrElse { return CantOpen(file, friendlyError(it)) })
}

internal fun friendlyError(t: Throwable): String = when (t) {
    is FileNotFoundException, is SecurityException ->
        "Papercraft can't reach this file anymore. It may have been moved or deleted."
    is TooBigException, is OutOfMemoryError -> "This file is too big to open for now."
    else -> t.message ?: "Something went wrong opening this file."
}

@Composable
private fun Spinner() = Box(Modifier.fillMaxSize(), Alignment.Center) {
    CircularProgressIndicator(Modifier.semantics { contentDescription = "Loading" })
}

@Composable
private fun CantOpen(file: DocFile, reason: String) {
    val context = LocalContext.current
    Column(
        Modifier.fillMaxSize().padding(32.dp).semantics { liveRegion = LiveRegionMode.Polite },
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(reason, textAlign = TextAlign.Center, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.height(20.dp))
        Button({ openElsewhere(context, file.uri) }) { Text("Open with another app") }
    }
}
