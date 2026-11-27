package org.papercraft

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.LocalActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.SystemBarStyle
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // A file handed to us by another app ("Open with"); only on first launch, not after recreation.
        val launchUri = intent?.data.takeIf { savedInstanceState == null }
        val settings = Settings(this)
        setContent {
            var mode by remember { mutableStateOf(settings.theme) }
            val dark = mode.isDark(isSystemInDarkTheme())
            // Status and navigation bar icons follow the app's theme, not the system's, when the two differ.
            DisposableEffect(dark) {
                val transparent = android.graphics.Color.TRANSPARENT
                val style = if (dark) SystemBarStyle.dark(transparent) else SystemBarStyle.light(transparent, transparent)
                enableEdgeToEdge(style, style)
                onDispose {}
            }
            PapercraftTheme(mode) { App(launchUri, mode) { mode = it; settings.theme = it } }
        }
    }
}

@Composable
fun App(launchUri: Uri?, themeMode: ThemeMode, onThemeChange: (ThemeMode) -> Unit) {
    val context = LocalContext.current
    val activity = LocalActivity.current
    val store = remember { Recents(context) }
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var recents by remember { mutableStateOf(store.load()) }
    var file by rememberSaveable(stateSaver = DocFileSaver) { mutableStateOf<DocFile?>(null) }
    var launching by rememberSaveable { mutableStateOf(launchUri != null) }
    var restored by rememberSaveable(stateSaver = DocFileSaver) { mutableStateOf<DocFile?>(null) }

    suspend fun openUri(uri: Uri) {
        // Keeps access after restart (read and write when the provider allows it), if the sending app allows it.
        context.keepAccess(uri)
        val (f, readable) = withContext(Dispatchers.IO) { context.describe(uri) to context.isReachable(uri) }
        // An unreadable file still opens (the viewer explains why) but stays out of Recent.
        if (readable) recents = store.add(f)
        file = f
    }

    LaunchedEffect(Unit) {
        if (!launching) return@LaunchedEffect
        launchUri?.let { openUri(it) }
        launching = false
    }

    fun openRecent(f: DocFile) = scope.launch {
        // Fresh size, version and write access: the stored ones are from the last time it was opened.
        val fresh = withContext(Dispatchers.IO) { if (context.isReachable(f.uri)) context.describe(f.uri) else null }
        if (fresh != null) {
            recents = store.add(fresh)
            file = fresh
        } else {
            val result = snackbar.showSnackbar(
                "Can't open \"${f.name}\". It was moved or deleted.",
                actionLabel = "Remove", duration = SnackbarDuration.Long,
            )
            if (result == SnackbarResult.ActionPerformed) recents = store.remove(f)
        }
    }

    fun back() {
        val intent = activity?.intent
        if (intent?.action == Intent.ACTION_VIEW && intent.data == file?.uri) activity.finish() else file = null
    }

    if (launching) {
        Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface))
        return
    }
    RecoveryPrompt { restored = it }
    val recovering = restored
    val recoveryEditor = recovering?.let { editorOf(it.ext) }
    if (recovering != null && recoveryEditor != null) return recoveryEditor.screen(recovering, true) { restored = null }
    AnimatedContent(
        targetState = file,
        contentKey = { it?.uri },
        transitionSpec = {
            if (targetState != null) (scaleIn(tween(260), initialScale = 0.96f) + fadeIn(tween(200))) togetherWith fadeOut(tween(120))
            else fadeIn(tween(200)) togetherWith (scaleOut(tween(220), targetScale = 0.96f) + fadeOut(tween(160)))
        },
        label = "screen",
    ) { current ->
        if (current == null) HomeScreen(
            recents = recents,
            snackbar = snackbar,
            themeMode = themeMode,
            onThemeChange = onThemeChange,
            onPick = { scope.launch { openUri(it) } },
            onOpen = { openRecent(it) },
            onRemove = { recents = store.remove(it) },
        )
        else ViewerScreen(current, onBack = ::back)
    }
}
