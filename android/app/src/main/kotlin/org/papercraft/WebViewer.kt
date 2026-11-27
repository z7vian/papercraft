package org.papercraft

import android.content.Context
import android.content.Intent
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import org.papercraft.core.WebResource
import org.papercraft.core.decodeText
import org.papercraft.core.parseMhtml
import java.io.ByteArrayInputStream

class WebPage(val html: String, val baseUrl: String?, val resources: Map<String, WebResource>)

fun Context.loadWebPage(file: DocFile) = WebPage(decodeText(readBytes(file)), null, emptyMap())

fun Context.loadWebArchive(file: DocFile): WebPage {
    val archive = parseMhtml(readBytes(file))
    return WebPage(archive.html, archive.baseUrl, archive.resources.flatMap { r -> r.urls.map { it to r } }.toMap())
}

@Composable
fun WebViewer(page: WebPage) {
    val context = LocalContext.current
    AndroidView(
        modifier = Modifier.fillMaxSize(),
        onRelease = { it.destroy() },
        factory = { ctx ->
            WebView(ctx).apply {
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                settings.builtInZoomControls = true
                settings.displayZoomControls = false
                webViewClient = object : WebViewClient() {
                    // Serve the archive's own files; block the internet so opening a page never phones home.
                    // The page itself arrives as data:, so only http(s) is blocked.
                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                        val r = page.resources[request.url.toString()]
                        return when {
                            r != null -> WebResourceResponse(r.mime, null, ByteArrayInputStream(r.data))
                            request.url.scheme in setOf("http", "https") ->
                                WebResourceResponse("text/plain", "utf-8", 403, "Blocked", emptyMap(), ByteArrayInputStream(ByteArray(0)))
                            else -> null
                        }
                    }

                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                        runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, request.url)) }
                        return true
                    }
                }
                loadDataWithBaseURL(page.baseUrl, page.html, "text/html", "utf-8", null)
            }
        },
    )
}
