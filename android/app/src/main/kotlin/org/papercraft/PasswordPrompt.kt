package org.papercraft

import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation

/**
 * Opens a PDF that may need a password. [content] loads with the current password (null at first), reloads when
 * `attempt` changes, and shows `locked` when the PDF refuses it. The password lives only in memory for this screen:
 * plain `remember`, so it is never written to saved state, disk or logs.
 */
@Composable
fun PdfUnlock(
    file: DocFile,
    content: @Composable (password: String?, attempt: Int, locked: @Composable (PasswordProtectedException) -> Unit) -> Unit,
) {
    var password by remember(file.uri) { mutableStateOf<String?>(null) }
    var attempt by remember(file.uri) { mutableIntStateOf(0) }
    var cancelled by remember(file.uri) { mutableStateOf(false) }
    content(password, attempt) { e ->
        if (cancelled) {
            CantOpen(file, "This PDF is password-protected.")
        } else {
            PasswordDialog(
                wrong = e.wrongPassword,
                onOpen = { password = it; attempt++ },
                onCancel = { password = null; cancelled = true },
            )
        }
    }
}

@Composable
private fun PasswordDialog(wrong: Boolean, onOpen: (String) -> Unit, onCancel: () -> Unit) {
    var typed by remember { mutableStateOf("") }
    var shown by remember { mutableStateOf(false) }
    val open = { if (typed.isNotEmpty()) onOpen(typed) }
    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text("Password needed") },
        text = {
            OutlinedTextField(
                typed, { typed = it },
                label = { Text("Password") },
                singleLine = true,
                isError = wrong,
                supportingText = if (wrong) ({ Text("That password doesn't open this PDF.") }) else null,
                visualTransformation = if (shown) VisualTransformation.None else PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Go),
                keyboardActions = KeyboardActions(onGo = { open() }),
                trailingIcon = { TextButton({ shown = !shown }) { Text(if (shown) "Hide" else "Show") } },
            )
        },
        confirmButton = { TextButton(open, enabled = typed.isNotEmpty()) { Text("Open") } },
        dismissButton = { TextButton(onCancel) { Text("Cancel") } },
    )
}
