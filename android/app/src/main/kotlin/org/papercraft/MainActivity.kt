package org.papercraft

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.LocalActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.runtime.Composable
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
        setContent { PapercraftTheme { App(launchUri) } }
    }
}

@Composable
fun App(launchUri: Uri?) {
    val context = LocalContext.current
    val activity = LocalActivity.current
    val store = remember { Recents(context) }
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var recents by remember { mutableStateOf(store.load()) }
    var file by rememberSaveable(stateSaver = DocFileSaver) { mutableStateOf<DocFile?>(null) }
    var launching by rememberSaveable { mutableStateOf(launchUri != null) }

    suspend fun openUri(uri: Uri) {
        // Keeps access after restart, if the sending app allows it.
        runCatching { context.contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
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
        if (withContext(Dispatchers.IO) { context.isReachable(f.uri) }) {
            recents = store.add(f)
            file = f
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
    AnimatedContent(
        targetState = file,
        contentKey = { it?.uri },
        transitionSpec = {
            if (targetState != null) (slideInHorizontally { it / 5 } + fadeIn()) togetherWith fadeOut()
            else fadeIn() togetherWith (slideOutHorizontally { it / 5 } + fadeOut())
        },
        label = "screen",
    ) { current ->
        if (current == null) HomeScreen(
            recents = recents,
            snackbar = snackbar,
            onPick = { scope.launch { openUri(it) } },
            onOpen = { openRecent(it) },
            onRemove = { recents = store.remove(it) },
        )
        else ViewerScreen(current, onBack = ::back)
    }
}
