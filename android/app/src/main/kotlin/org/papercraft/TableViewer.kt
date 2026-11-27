package org.papercraft

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.roundToInt
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.papercraft.core.parseCsv
import org.papercraft.core.parseDbf
import org.papercraft.core.parseDif

fun Context.loadTable(file: DocFile): List<List<String>> {
    val bytes = readBytes(file, MAX_TEXT_BYTES)
    return when (file.ext) {
        "dbf" -> parseDbf(bytes)
        "dif" -> parseDif(bytes)
        else -> parseCsv(bytes)
    }
}

/** A table: column letters on top, row numbers on the left. [find] shades the matching cells and scrolls to the current one. */
@Composable
fun TableViewer(rows: List<List<String>>, find: FindState) {
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
    val list = rememberLazyListState()
    val across = rememberScrollState()
    val hits = rememberHits<CellHit>(find, rows, startAt = { it.firstAtOrAfter(list.firstVisibleItemIndex) { hit -> hit.row } }) { query ->
        withContext(Dispatchers.Default) { findInTable(rows, cols, query) }
    }
    val hitAt = remember(hits) { hits.indices.associateBy { cellKey(hits[it].row, hits[it].col) } }
    LaunchedEffect(hits, find.moves) {
        val hit = hits.getOrNull(find.current) ?: return@LaunchedEffect
        val info = list.layoutInfo
        val item = info.visibleItemsInfo.firstOrNull { it.index == hit.row + 1 }
        val headerPx = with(density) { rowH.toPx() }
        if (item == null || item.offset < info.viewportStartOffset + headerPx || item.offset + item.size > info.viewportEndOffset) {
            list.scrollToItem(tableJumpItem(hit.row))
        }
        val left = with(density) { (numW + cellW * hit.col).toPx() }.toDouble()
        val width = with(density) { cellW.toPx() }.toDouble()
        across.scrollTo(revealSpan(left, left + width, across.value.toDouble(), across.viewportSize.toDouble()).roundToInt())
    }

    @Composable
    fun Cell(text: String, width: Dp, header: Boolean = false, hit: Int = -1) {
        val numeric = !header && text.trim().toDoubleOrNull() != null
        val colors = MaterialTheme.colorScheme
        val found = hit >= 0
        val shown = found && hit == find.current
        Box(
            Modifier.width(width).height(rowH)
                .background(
                    when {
                        shown -> colors.primary
                        found -> colors.primaryContainer
                        header -> headerBg
                        else -> Color.Transparent
                    },
                )
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
                color = when {
                    shown -> colors.onPrimary
                    found -> colors.onPrimaryContainer
                    header -> colors.onSurfaceVariant
                    else -> colors.onSurface
                },
            )
        }
    }

    Box(Modifier.fillMaxSize().horizontalScroll(across)) {
        LazyColumn(Modifier.width(numW + cellW * cols).fillMaxHeight(), state = list) {
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
                    for (c in 0 until cols) Cell(row.getOrElse(c) { "" }, cellW, hit = if (hits.isEmpty()) -1 else hitAt[cellKey(i, c)] ?: -1)
                }
            }
        }
    }
}

/** The list item to scroll to so table row [row] shows just under the column letters: the column letters are item 0
 * and stay pinned, so the item before the row's goes under them. */
fun tableJumpItem(row: Int): Int = row

private fun cellKey(row: Int, col: Int): Long = (row.toLong() shl 32) or col.toLong()

/** Spreadsheet column letters: 0 is A, 26 is AA. */
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
