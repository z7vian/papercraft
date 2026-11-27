package org.papercraft

import android.content.Context
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import org.papercraft.core.decodeText

// Limit: text/CSV become ~5x their size in memory; stream them to lift this.
internal const val MAX_TEXT_BYTES = 30_000_000L

// Giant lines (minified JSON, logs) are split so no single Text has to lay out megabytes.
fun Context.loadText(file: DocFile): List<String> =
    decodeText(readBytes(file, MAX_TEXT_BYTES)).lines().flatMap { it.chunked(4000).ifEmpty { listOf("") } }

@Composable
fun TextViewer(lines: List<String>) {
    SelectionContainer {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(horizontal = 20.dp, vertical = 16.dp)) {
            items(lines) { Text(it, style = MaterialTheme.typography.bodyLarge) }
        }
    }
}
