package org.papercraft

import android.net.Uri
import android.text.format.DateUtils
import android.text.format.Formatter
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    recents: List<DocFile>,
    snackbar: SnackbarHostState,
    onPick: (Uri) -> Unit,
    onOpen: (DocFile) -> Unit,
    onRemove: (DocFile) -> Unit,
) {
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { it?.let(onPick) }
    var filter by rememberSaveable { mutableStateOf<Category?>(null) }
    val shown = remember(recents, filter) { recents.filter { filter == null || categoryOf(it.ext) == filter } }
    val scroll = TopAppBarDefaults.pinnedScrollBehavior()
    // A snackbar left showing when this screen goes away never times out and blocks every later one.
    DisposableEffect(Unit) { onDispose { snackbar.currentSnackbarData?.dismiss() } }

    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection).semantics { paneTitle = "Papercraft" },
        topBar = { TopAppBar(title = { Text("Papercraft", fontWeight = FontWeight.SemiBold) }, scrollBehavior = scroll) },
        snackbarHost = { SnackbarHost(snackbar) },
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = { picker.launch(arrayOf("*/*")) },
                icon = { Icon(painterResource(R.drawable.ic_add), null) },
                text = { Text("Open file") },
            )
        },
    ) { padding ->
        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = PaddingValues(
                top = padding.calculateTopPadding(),
                bottom = padding.calculateBottomPadding() + 88.dp, // room for the button
            ),
        ) {
            if (recents.isNotEmpty()) item {
                Row(
                    Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    FilterChip(filter == null, { filter = null }, label = { Text("All") })
                    Category.entries.forEach { c ->
                        FilterChip(filter == c, { filter = if (filter == c) null else c }, label = { Text(c.label) })
                    }
                }
            }
            if (shown.isEmpty()) item { EmptyState(filter, Modifier.fillParentMaxHeight(0.8f)) }
            shown.groupBy { dateGroup(it.openedAt) }.forEach { (label, files) ->
                item(key = "header-$label") { SectionHeader(label) }
                items(files, key = { it.uri.toString() }) { file ->
                    RecentRow(file, onOpen = { onOpen(file) }, onRemove = { onRemove(file) })
                }
            }
        }
    }
}

private fun dateGroup(time: Long): String = when {
    DateUtils.isToday(time) -> "Today"
    DateUtils.isToday(time + DateUtils.DAY_IN_MILLIS) -> "Yesterday"
    System.currentTimeMillis() - time < 7 * DateUtils.DAY_IN_MILLIS -> "This week"
    else -> "Earlier"
}

@Composable
private fun SectionHeader(label: String) = Text(
    label,
    Modifier.padding(start = 16.dp, top = 20.dp, bottom = 4.dp).semantics { heading() },
    style = MaterialTheme.typography.titleSmall,
    color = MaterialTheme.colorScheme.primary,
)

@Composable
private fun RecentRow(file: DocFile, onOpen: () -> Unit, onRemove: () -> Unit) {
    val context = LocalContext.current
    var menu by remember { mutableStateOf(false) }
    val details = listOfNotNull(
        if (System.currentTimeMillis() - file.openedAt < DateUtils.MINUTE_IN_MILLIS) "Just now"
        else DateUtils.getRelativeTimeSpanString(file.openedAt).toString(),
        file.size.takeIf { it >= 0 }?.let { Formatter.formatShortFileSize(context, it) },
    ).joinToString(" · ")
    ListItem(
        modifier = Modifier
            .clickable(onClickLabel = "Open", onClick = onOpen)
            .semantics { customActions = listOf(CustomAccessibilityAction("Remove from recent") { onRemove(); true }) },
        leadingContent = { FileBadge(file.ext) },
        supportingContent = { Text(details) },
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
private fun EmptyState(filter: Category?, modifier: Modifier) {
    Column(
        modifier.fillMaxWidth().padding(horizontal = 32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            Modifier.size(120.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primaryContainer),
            contentAlignment = Alignment.Center,
        ) {
            Image(painterResource(R.drawable.ic_launcher_foreground), null, Modifier.requiredSize(160.dp))
        }
        Spacer(Modifier.height(24.dp))
        Text(
            if (filter == null) "No files yet" else "No recent ${filter.label} files",
            style = MaterialTheme.typography.titleLarge,
        )
        Text(
            "Open $openableHint.",
            Modifier.padding(top = 8.dp),
            textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
