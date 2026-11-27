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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
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
import org.papercraft.core.parseCsv

fun Context.loadTable(file: DocFile): List<List<String>> = parseCsv(readBytes(file, MAX_TEXT_BYTES))

@Composable
fun TableViewer(rows: List<List<String>>) {
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
