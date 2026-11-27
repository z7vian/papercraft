package org.papercraft

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** The office family a file belongs to, for filters and badges. White text on each color is ≥4.5:1. */
enum class Category(val label: String, val color: Color, val exts: Set<String>) {
    Docs("Docs", Color(0xFF2B579A), setOf(
        "doc", "docx", "docm", "dot", "dotx", "dotm", "wps", "wpt", "txt", "rtf", "xml", "html", "htm", "mht", "mhtml",
    )),
    Sheets("Sheets", Color(0xFF217346), setOf(
        "xls", "xlsx", "xlsm", "xlt", "xltx", "xltm", "xla", "xlam", "et", "ett", "csv", "dbf", "dif", "prn",
    )),
    Slides("Slides", Color(0xFFD35400), setOf(
        "ppt", "pptx", "pptm", "pot", "potx", "potm", "pps", "ppsx", "ppsm", "dps", "dpt", "ts",
    )),
    Pdf("PDF", Color(0xFFD93025), setOf("pdf")),
    Images("Images", Color(0xFF7B1FA2), setOf("jpg", "jpeg", "png", "bmp", "tif", "tiff")),
}

/**
 * How a file is shown. [noun] names what it opens, for the home screen hint (listed in this order).
 * The "open with" intent filter in AndroidManifest.xml lists the MIME types of these extensions.
 */
enum class Viewer(val exts: Set<String>, val noun: String, val darkBackdrop: Boolean = false) {
    Pdf(setOf("pdf"), "PDF", darkBackdrop = true),
    Image(setOf("jpg", "jpeg", "png", "bmp"), "picture", darkBackdrop = true),
    Tiff(setOf("tif", "tiff"), "picture", darkBackdrop = true),
    Table(setOf("csv"), "CSV spreadsheet"),
    Text(setOf("txt"), "text file"),
    Web(setOf("html", "htm"), "web page"),
    WebArchive(setOf("mht", "mhtml"), "saved web page"),
}

/** "a X, Y or Z", from each viewer's [Viewer.noun]. */
val openableHint: String = Viewer.entries.map { it.noun }.distinct().let { "a " + it.dropLast(1).joinToString(", ") + " or " + it.last() }

fun categoryOf(ext: String): Category? = Category.entries.firstOrNull { ext in it.exts }

fun viewerOf(ext: String): Viewer? = Viewer.entries.firstOrNull { ext in it.exts }

/** Colored tile with the extension, like WPS/Office file icons. Decorative: the row text already says the type. */
@Composable
fun FileBadge(ext: String, size: Dp = 44.dp) {
    Box(
        Modifier.size(size).clip(RoundedCornerShape(size * 0.23f))
            .background(categoryOf(ext)?.color ?: Color(0xFF6B6B6B))
            .clearAndSetSemantics {},
        contentAlignment = Alignment.Center,
    ) {
        Text(
            ext.take(4).uppercase().ifEmpty { "?" },
            color = Color.White, fontWeight = FontWeight.Bold, maxLines = 1, softWrap = false,
            // Sized in dp on purpose so it can't overflow the tile at large font settings.
            fontSize = with(LocalDensity.current) { (size * 0.27f).toSp() },
        )
    }
}
