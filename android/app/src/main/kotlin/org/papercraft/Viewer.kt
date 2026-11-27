package org.papercraft

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
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
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Color
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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import java.io.FileNotFoundException

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
        val backdrop = if (viewer?.darkBackdrop == true) MaterialTheme.colorScheme.surfaceContainerHighest else Color.Transparent
        Box(Modifier.fillMaxSize().background(backdrop).padding(padding).clipToBounds()) {
            when (viewer) {
                Viewer.Text -> Loaded(file, { loadText(file) }) { TextViewer(it) }
                Viewer.Table -> Loaded(file, { loadTable(file) }) { TableViewer(it) }
                Viewer.Image -> Loaded(file, { loadImage(file) }) { ImageViewer(it, file.name) }
                Viewer.Tiff -> Loaded(file, { loadTiff(file) }) { ImageViewer(it, file.name) }
                Viewer.Web -> Loaded(file, { loadWebPage(file) }) { WebViewer(it) }
                Viewer.WebArchive -> Loaded(file, { loadWebArchive(file) }) { WebViewer(it) }
                Viewer.Pdf -> Loaded(file, { PdfDoc.open(this, file.uri) }, release = PdfDoc::closeLater) { PdfViewer(it) }
                null -> CantOpen(
                    file,
                    if (categoryOf(file.ext) != null) "Papercraft can't open .${file.ext} files yet. Support is coming in a later version."
                    else "Papercraft can't open ${if (file.ext.isEmpty()) "this kind of" else ".${file.ext}"} files.",
                )
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

private fun friendlyError(t: Throwable): String = when (t) {
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
