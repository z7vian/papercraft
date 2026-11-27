package org.papercraft

import android.net.Uri
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.foundation.text.input.clearText
import androidx.compose.foundation.text.input.rememberTextFieldState
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    recents: List<DocFile>,
    snackbar: SnackbarHostState,
    themeMode: ThemeMode,
    onThemeChange: (ThemeMode) -> Unit,
    onPick: (Uri) -> Unit,
    onOpen: (DocFile) -> Unit,
    onRemove: (DocFile) -> Unit,
) {
    val context = LocalContext.current
    val settings = remember { Settings(context) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { it?.let(onPick) }
    var filter by rememberSaveable { mutableStateOf<Category?>(null) }
    val search = rememberTextFieldState()
    var asGrid by rememberSaveable { mutableStateOf(settings.recentsAsGrid) }
    var showSettings by remember { mutableStateOf(false) }
    val counts = remember(recents) { recents.groupingBy { categoryOf(it.ext) }.eachCount() }
    val query = search.text.toString().trim()
    val shown = remember(recents, filter, query) {
        recents.filter { (filter == null || categoryOf(it.ext) == filter) && it.name.contains(query, ignoreCase = true) }
    }
    // A snackbar left showing when this screen goes away never times out and blocks every later one.
    DisposableEffect(Unit) { onDispose { snackbar.currentSnackbarData?.dismiss() } }

    Scaffold(
        modifier = Modifier.semantics { paneTitle = "Papercraft" },
        containerColor = MaterialTheme.colorScheme.surfaceContainer,
        snackbarHost = { SnackbarHost(snackbar) },
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = { picker.launch(arrayOf("*/*")) },
                icon = { Icon(painterResource(R.drawable.ic_add), null) },
                text = { Text("Open file") },
                shape = MaterialTheme.shapes.medium,
                containerColor = MaterialTheme.colorScheme.primary,
                contentColor = MaterialTheme.colorScheme.onPrimary,
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(top = padding.calculateTopPadding())) {
            Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Papercraft", Modifier.weight(1f).semantics { heading() }, style = MaterialTheme.typography.titleLarge)
                IconButton({ showSettings = true }) { Icon(painterResource(R.drawable.ic_tune), "Settings") }
            }
            if (recents.isNotEmpty()) SearchField(search, Modifier.padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 16.dp))
            else Spacer(Modifier.height(12.dp))
            // The sheet: content sits on one raised layer above the tinted backdrop.
            Surface(
                Modifier.fillMaxSize(),
                shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
                color = MaterialTheme.colorScheme.surface,
            ) {
                LazyVerticalGrid(
                    columns = GridCells.Adaptive(148.dp),
                    contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 20.dp, bottom = padding.calculateBottomPadding() + 96.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    if (recents.isEmpty()) {
                        item(span = { GridItemSpan(maxLineSpan) }) { FirstRun(Modifier.padding(top = 48.dp)) }
                        return@LazyVerticalGrid
                    }
                    item(span = { GridItemSpan(maxLineSpan) }) { TypeTiles(counts, filter) { filter = if (filter == it) null else it } }
                    item(span = { GridItemSpan(maxLineSpan) }) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                filter?.let { "Recent ${it.label}" } ?: "Recent",
                                Modifier.weight(1f).semantics { heading() },
                                style = MaterialTheme.typography.titleMedium,
                            )
                            IconButton({ asGrid = !asGrid; settings.recentsAsGrid = asGrid }) {
                                Icon(
                                    painterResource(if (asGrid) R.drawable.ic_list else R.drawable.ic_grid),
                                    if (asGrid) "Show as list" else "Show as previews",
                                )
                            }
                        }
                    }
                    when {
                        shown.isEmpty() -> item(span = { GridItemSpan(maxLineSpan) }) {
                            NoMatches { filter = null; search.clearText() }
                        }
                        asGrid -> items(shown, key = { it.uri.toString() }) { file ->
                            PreviewCard(file, onOpen = { onOpen(file) }, onRemove = { onRemove(file) }, Modifier.animateItem())
                        }
                        else -> items(shown, key = { it.uri.toString() }, span = { GridItemSpan(maxLineSpan) }) { file ->
                            RecentRow(file, onOpen = { onOpen(file) }, onRemove = { onRemove(file) }, Modifier.animateItem())
                        }
                    }
                }
            }
        }
    }
    if (showSettings) SettingsSheet(themeMode, onThemeChange) { showSettings = false }
}

@Composable
private fun SearchField(search: TextFieldState, modifier: Modifier) {
    Surface(modifier.fillMaxWidth().height(52.dp), shape = CircleShape, color = MaterialTheme.colorScheme.surfaceContainerHighest) {
        Row(Modifier.padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(painterResource(R.drawable.ic_search), null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Box(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                if (search.text.isEmpty()) {
                    Text("Search recent files", style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                BasicTextField(
                    search,
                    Modifier.fillMaxWidth().semantics { contentDescription = "Search recent files" },
                    lineLimits = TextFieldLineLimits.SingleLine,
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                    cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                )
            }
            if (search.text.isNotEmpty()) IconButton({ search.clearText() }) { Icon(painterResource(R.drawable.ic_close), "Clear search") }
        }
    }
}

@Composable
private fun TypeTiles(counts: Map<Category?, Int>, selected: Category?, onSelect: (Category) -> Unit) {
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Category.entries.forEach { c ->
            val on = selected == c
            Surface(
                selected = on,
                onClick = { onSelect(c) },
                modifier = Modifier.width(104.dp),
                shape = MaterialTheme.shapes.medium,
                color = if (on) c.color else MaterialTheme.colorScheme.surfaceContainerLow,
                contentColor = if (on) Color.White else MaterialTheme.colorScheme.onSurface,
            ) {
                Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Box(Modifier.size(10.dp).clip(CircleShape).background(if (on) Color.White else c.color))
                    Column {
                        Text(c.label, style = MaterialTheme.typography.titleSmall)
                        val n = counts[c] ?: 0
                        Text(
                            "$n recent",
                            style = MaterialTheme.typography.bodySmall,
                            color = if (on) Color.White.copy(alpha = 0.85f) else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

private fun details(context: android.content.Context, file: DocFile) = listOfNotNull(
    typeLabel(file.ext),
    if (System.currentTimeMillis() - file.openedAt < DateUtils.MINUTE_IN_MILLIS) "Just now"
    else DateUtils.getRelativeTimeSpanString(file.openedAt).toString(),
    file.size.takeIf { it >= 0 }?.let { Formatter.formatShortFileSize(context, it) },
).joinToString(" · ")

/** A recent file as its first page: the preview saved when it was last opened, else its type badge. */
@Composable
private fun PreviewCard(file: DocFile, onOpen: () -> Unit, onRemove: () -> Unit, modifier: Modifier) {
    val context = LocalContext.current
    var menu by remember { mutableStateOf(false) }
    val thumb by produceState<android.graphics.Bitmap?>(null, file.uri, file.openedAt) {
        value = withContext(Dispatchers.IO) { Thumbnails.load(context, file.uri) }
    }
    Box(modifier) {
        Column(
            Modifier.clip(MaterialTheme.shapes.medium)
                .combinedClickable(onClickLabel = "Open", onClick = onOpen, onLongClickLabel = "More options", onLongClick = { menu = true })
                .semantics { customActions = listOf(CustomAccessibilityAction("Remove from recent") { onRemove(); true }) },
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Box(
                Modifier.fillMaxWidth().aspectRatio(0.78f).clip(MaterialTheme.shapes.medium)
                    .background(MaterialTheme.colorScheme.surfaceContainerHigh),
                contentAlignment = Alignment.Center,
            ) {
                AnimatedContent(thumb, transitionSpec = { fadeIn(tween(180)) togetherWith fadeOut(tween(120)) }, label = "thumb") { bmp ->
                    if (bmp != null) {
                        Box(Modifier.fillMaxSize()) {
                            Image(bmp.asImageBitmap(), null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop, alignment = Alignment.TopCenter)
                            FileBadge(file.ext, size = 28.dp, modifier = Modifier.align(Alignment.BottomStart).padding(8.dp))
                        }
                    } else {
                        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { FileBadge(file.ext, size = 56.dp) }
                    }
                }
            }
            Column(Modifier.padding(horizontal = 4.dp)) {
                Text(file.name, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(details(context, file), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
            }
        }
        DropdownMenu(menu, { menu = false }) {
            DropdownMenuItem(text = { Text("Remove from recent") }, onClick = { menu = false; onRemove() })
        }
    }
}

@Composable
private fun RecentRow(file: DocFile, onOpen: () -> Unit, onRemove: () -> Unit, modifier: Modifier) {
    val context = LocalContext.current
    var menu by remember { mutableStateOf(false) }
    ListItem(
        modifier = modifier.clip(MaterialTheme.shapes.small)
            .clickable(onClickLabel = "Open", onClick = onOpen)
            .semantics { customActions = listOf(CustomAccessibilityAction("Remove from recent") { onRemove(); true }) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        leadingContent = { FileBadge(file.ext) },
        supportingContent = { Text(details(context, file)) },
        trailingContent = {
            Box {
                IconButton({ menu = true }) { Icon(painterResource(R.drawable.ic_more), "More options for ${file.name}") }
                DropdownMenu(menu, { menu = false }) {
                    DropdownMenuItem(text = { Text("Remove from recent") }, onClick = { menu = false; onRemove() })
                }
            }
        },
    ) { Text(file.name, maxLines = 1, overflow = TextOverflow.Ellipsis) }
}

@Composable
private fun FirstRun(modifier: Modifier) {
    Column(modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Box(
            Modifier.size(120.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primaryContainer),
            contentAlignment = Alignment.Center,
        ) {
            Image(painterResource(R.drawable.ic_launcher_foreground), null, Modifier.requiredSize(160.dp))
        }
        Spacer(Modifier.height(24.dp))
        Text("No files yet", style = MaterialTheme.typography.titleLarge)
        Text(
            "Open $openableHint.",
            Modifier.padding(top = 8.dp),
            textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun NoMatches(onClear: () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(top = 32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Text("No recent files match", style = MaterialTheme.typography.titleMedium)
        TextButton(onClear, Modifier.padding(top = 4.dp)) { Text("Clear search and filter") }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SettingsSheet(mode: ThemeMode, onMode: (ThemeMode) -> Unit, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismiss) {
        Column(Modifier.padding(start = 24.dp, end = 24.dp, bottom = 32.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Theme", style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                ThemeMode.entries.forEachIndexed { i, m ->
                    SegmentedButton(
                        selected = m == mode,
                        onClick = { onMode(m) },
                        shape = SegmentedButtonDefaults.itemShape(i, ThemeMode.entries.size),
                    ) { Text(m.label, maxLines = 1) }
                }
            }
            Text(
                "Black turns the background fully off on OLED screens.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}
