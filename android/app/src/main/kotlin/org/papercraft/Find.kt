package org.papercraft

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withContext

// Searching waits this long after the last keystroke, so typing a word runs one search, not one per letter.
private const val TYPING_PAUSE_MS = 250L

/** Find in file, shared by the find bar (query, "n of m", previous/next) and the viewer that searches and shows the hits. */
@Stable
class FindState {
    var open by mutableStateOf(false)
        private set
    var query by mutableStateOf("")
    var count by mutableIntStateOf(0)
        private set
    /** Index of the hit shown, -1 for none. */
    var current by mutableIntStateOf(-1)
        private set
    /** Bumped on every move, even back to the same hit, so the viewer scrolls to it again. */
    var moves by mutableIntStateOf(0)
        private set
    private var searching by mutableStateOf(false)
    private var failed by mutableStateOf(false)
    private var pagesOnly by mutableStateOf(false)

    val label: String? get() = findLabel(query, searching, failed, current, count, count >= MAX_FIND_HITS, pagesOnly)

    fun show() { open = true }
    fun close() { open = false; query = "" }
    fun next() = moveTo(nextHit(current, count))
    fun previous() = moveTo(previousHit(current, count))

    private fun moveTo(hit: Int) {
        if (hit < 0) return
        current = hit
        moves++
    }

    internal fun started() {
        count = 0; current = -1; failed = false; pagesOnly = false
        searching = query.isNotEmpty()
    }

    internal fun finished(count: Int, first: Int, pagesOnly: Boolean) {
        this.count = count; this.pagesOnly = pagesOnly; searching = false
        moveTo(first)
    }

    internal fun fail() { searching = false; failed = true }
}

/**
 * Runs [search] off the main thread for each query typed into [find], once typing pauses; a new query or [source] cancels it.
 * [startAt] picks the first hit to show (the first one from where the reader is); [pagesOnly] says the hits only name pages.
 */
@Composable
fun <T> rememberHits(
    find: FindState,
    source: Any,
    startAt: (List<T>) -> Int,
    pagesOnly: (List<T>) -> Boolean = { false },
    search: suspend (String) -> List<T>,
): List<T> {
    var hits by remember(source) { mutableStateOf(emptyList<T>()) }
    val query = find.query
    LaunchedEffect(source, query) {
        hits = emptyList()
        find.started()
        if (query.isEmpty()) return@LaunchedEffect
        delay(TYPING_PAUSE_MS)
        val found = try {
            search(query)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            return@LaunchedEffect find.fail()
        }
        hits = found
        find.finished(found.size, startAt(found), pagesOnly(found))
    }
    return hits
}

/** Replaces the viewer's top bar while finding: query field, "n of m", previous, next and close. Back closes it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FindBar(find: FindState, windowInsets: WindowInsets) {
    BackHandler(onBack = find::close)
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    val hasHits = find.count > 0
    TopAppBar(
        title = {
            TextField(
                find.query, { find.query = it },
                Modifier.fillMaxWidth().focusRequester(focus),
                textStyle = MaterialTheme.typography.bodyLarge,
                placeholder = { Text("Find in file") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { find.next() }),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = Color.Transparent, unfocusedContainerColor = Color.Transparent,
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                ),
            )
        },
        actions = {
            find.label?.let {
                Text(
                    it, Modifier.padding(horizontal = 4.dp).semantics { liveRegion = LiveRegionMode.Polite },
                    style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1,
                )
            }
            IconButton(find::previous, enabled = hasHits) { Icon(painterResource(R.drawable.ic_find_previous), "Previous match") }
            IconButton(find::next, enabled = hasHits) { Icon(painterResource(R.drawable.ic_find_next), "Next match") }
            IconButton(find::close) { Icon(painterResource(R.drawable.ic_close), "Close find") }
        },
        windowInsets = windowInsets,
        colors = TopAppBarDefaults.topAppBarColors(
            containerColor = MaterialTheme.colorScheme.surfaceContainer,
            scrolledContainerColor = MaterialTheme.colorScheme.surfaceContainer,
        ),
    )
}

/** Boxes over a drawn page for the hits [onPage] (indices into [hits]); the [current] one is drawn stronger. */
@Composable
fun PageHighlights(hits: List<PageHit>, onPage: List<Int>, current: Int) {
    val match = MaterialTheme.colorScheme.primary.copy(alpha = 0.25f)
    val shown = MaterialTheme.colorScheme.primary.copy(alpha = 0.5f)
    Canvas(Modifier.fillMaxSize()) {
        for (h in onPage) if (h != current) drawHit(hits[h], match)
        if (current in onPage) drawHit(hits[current], shown)
    }
}

private fun DrawScope.drawHit(hit: PageHit, color: Color) {
    for (box in hit.boxes) {
        val px = box.scaled(size.width, size.height)
        drawRect(color, Offset(px.left, px.top), Size(px.right - px.left, px.bottom - px.top))
    }
}

/** Find in a [TextViewer]: the hits, which lines hold them, and the layouts of shown lines that hold one, for scrolling to a match. */
@Stable
class TextFind internal constructor(val find: FindState, val hits: List<TextHit>, internal val layouts: MutableMap<Int, TextLayoutResult>) {
    internal val byLine: Map<Int, List<Int>> = hits.indices.groupBy { hits[it].line }
}

/** Searches [lines] for [find]'s query and scrolls [list] so the current match is on screen. */
@Composable
fun rememberTextFind(find: FindState, lines: List<String>, list: LazyListState): TextFind {
    val hits = rememberHits<TextHit>(find, lines, startAt = { it.firstAtOrAfter(list.firstVisibleItemIndex) { hit -> hit.line } }) { query ->
        withContext(Dispatchers.Default) { findInLines(lines, query) }
    }
    // Kept across searches: a line whose text didn't change isn't laid out again, so its old layout must stay.
    val layouts = remember(lines) { mutableStateMapOf<Int, TextLayoutResult>() }
    val found = remember(hits) { TextFind(find, hits, layouts) }
    LaunchedEffect(found, find.moves) {
        val hit = hits.getOrNull(find.current) ?: return@LaunchedEffect
        val info = list.layoutInfo
        val item = info.visibleItemsInfo.firstOrNull { it.index == hit.line }
        val layout = found.layouts[hit.line]
        if (item != null && layout != null) {
            val row = layout.getLineForOffset(hit.start)
            // Already on screen: leave the text where it is.
            if (item.offset + layout.getLineTop(row) >= info.viewportStartOffset && item.offset + layout.getLineBottom(row) <= info.viewportEndOffset) {
                return@LaunchedEffect
            }
        }
        // Long lines wrap over many rows, so bring the line in first, then scroll to the match's row within it.
        list.scrollToItem(hit.line)
        val laidOut = snapshotFlow { found.layouts[hit.line] }.filterNotNull().first()
        val top = laidOut.getLineTop(laidOut.getLineForOffset(hit.start)).toInt()
        list.scrollToItem(hit.line, (top - list.layoutInfo.viewportSize.height / 3).coerceAtLeast(0))
    }
    return found
}

/** Line [index] of a text file with its matches highlighted; the current match is drawn stronger. */
@Composable
fun FoundLine(found: TextFind, index: Int, line: String, style: TextStyle, softWrap: Boolean = true) {
    val mine = found.byLine[index] ?: return Text(line, style = style, softWrap = softWrap)
    val current = found.find.current
    val colors = MaterialTheme.colorScheme
    val text = remember(line, mine, current, colors) {
        val match = SpanStyle(background = colors.primaryContainer, color = colors.onPrimaryContainer)
        val shown = SpanStyle(background = colors.primary, color = colors.onPrimary)
        AnnotatedString.Builder(line).apply {
            for (h in mine) if (h != current) found.hits[h].let { addStyle(match, it.start, it.end) }
            if (current in mine) found.hits[current].let { addStyle(shown, it.start, it.end) }
        }.toAnnotatedString()
    }
    DisposableEffect(found.layouts, index) { onDispose { found.layouts.remove(index) } }
    Text(text, style = style, softWrap = softWrap, onTextLayout = { found.layouts[index] = it })
}
