package org.papercraft

import android.content.Context
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import org.papercraft.core.decodeText
import org.papercraft.core.formatXml
import org.papercraft.core.parsePrn
import org.papercraft.core.parseRtf

// Limit: text/CSV become ~5x their size in memory; stream them to lift this.
internal const val MAX_TEXT_BYTES = 30_000_000L

/** Lines to show for a .txt, .rtf (paragraphs), .xml (indented when well-formed) or .prn (printed layout) file. */
fun Context.loadText(file: DocFile): List<String> {
    val bytes = readBytes(file, MAX_TEXT_BYTES)
    val lines = when (file.ext) {
        "rtf" -> parseRtf(bytes)
        "xml" -> formatXml(bytes)
        "prn" -> parsePrn(bytes)
        else -> decodeText(bytes).lines()
    }
    // Giant lines (minified JSON, logs) are split so no single Text has to lay out megabytes.
    return lines.flatMap { it.chunked(4000).ifEmpty { listOf("") } }
}

/** [monospace] keeps space-aligned columns (print files) lined up; [find]'s matches are highlighted and scrolled to. */
@Composable
fun TextViewer(lines: List<String>, find: FindState, monospace: Boolean = false) {
    val style = MaterialTheme.typography.bodyLarge.let { if (monospace) it.copy(fontFamily = FontFamily.Monospace) else it }
    val list = rememberLazyListState()
    val found = rememberTextFind(find, lines, list)
    SelectionContainer {
        LazyColumn(Modifier.fillMaxSize(), state = list, contentPadding = PaddingValues(horizontal = 20.dp, vertical = 16.dp)) {
            itemsIndexed(lines) { i, line -> FoundLine(found, i, line, style) }
        }
    }
}
