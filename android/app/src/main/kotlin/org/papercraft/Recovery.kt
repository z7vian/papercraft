package org.papercraft

import android.text.format.DateUtils
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * On launch, lists the unsaved edits earlier sessions left behind ([Drafts.pending]): Restore opens one in its editor
 * through [onRestore] (the editor then reads the draft), Discard drops it. Asks once per launch, not after recreation.
 */
@Composable
fun RecoveryPrompt(onRestore: (DocFile) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var done by rememberSaveable { mutableStateOf(false) }
    var drafts by remember { mutableStateOf<List<Draft>?>(null) }
    LaunchedEffect(done) {
        if (done) return@LaunchedEffect
        val pending = withContext(Dispatchers.IO) { runCatching { Drafts(context).pending() }.getOrDefault(emptyList()) }
        if (pending.isEmpty()) done = true else drafts = pending
    }
    val list = drafts
    if (done || list.isNullOrEmpty()) return

    fun discard(draft: Draft) {
        drafts = list - draft
        if (list.size == 1) done = true
        scope.launch(Dispatchers.IO) { runCatching { Drafts(context).discard(draft.file.uri) } }
    }

    AlertDialog(
        onDismissRequest = { done = true },
        title = { Text("Unsaved changes") },
        text = {
            Column(Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState())) {
                Text(
                    if (list.size == 1) "Papercraft closed before these changes were saved." else "Papercraft closed before changes to these files were saved.",
                    style = MaterialTheme.typography.bodyMedium,
                )
                list.forEach { draft ->
                    Column(Modifier.fillMaxWidth().padding(top = 16.dp)) {
                        Text(draft.file.name, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            "Edited " + DateUtils.getRelativeTimeSpanString(draft.savedAt),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Row(Modifier.align(Alignment.End), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            TextButton({ discard(draft) }) { Text("Discard") }
                            if (editorOf(draft.file.ext) != null) {
                                TextButton({ done = true; onRestore(draft.file) }) { Text("Restore") }
                            }
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton({ done = true }) { Text("Later") } },
    )
}
