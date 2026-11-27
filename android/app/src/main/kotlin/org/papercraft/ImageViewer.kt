package org.papercraft

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.ExifInterface
import androidx.compose.foundation.Image
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntSize
import org.papercraft.core.decodeTiff
import java.io.ByteArrayInputStream
import java.nio.ByteBuffer

private const val MAX_IMAGE_SIDE = 4096 // bigger bitmaps crash the Canvas

fun Context.loadImage(file: DocFile): ImageBitmap {
    val bytes = readBytes(file)
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / sample > MAX_IMAGE_SIDE) sample *= 2
    val bmp = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
        ?: error("This picture looks damaged.")
    // Phone photos are stored sideways with an EXIF "rotate me" tag.
    val degrees = when (ExifInterface(ByteArrayInputStream(bytes)).getAttributeInt(ExifInterface.TAG_ORIENTATION, 0)) {
        ExifInterface.ORIENTATION_ROTATE_90 -> 90f
        ExifInterface.ORIENTATION_ROTATE_180 -> 180f
        ExifInterface.ORIENTATION_ROTATE_270 -> 270f
        else -> 0f
    }
    val upright = if (degrees == 0f) bmp
    else Bitmap.createBitmap(bmp, 0, 0, bmp.width, bmp.height, Matrix().apply { postRotate(degrees) }, true)
    return upright.asImageBitmap()
}

// Android can't decode TIFF; the Rust core does.
fun Context.loadTiff(file: DocFile): ImageBitmap {
    val img = decodeTiff(readBytes(file), MAX_IMAGE_SIDE.toUInt())
    return Bitmap.createBitmap(img.width.toInt(), img.height.toInt(), Bitmap.Config.ARGB_8888)
        .apply { copyPixelsFromBuffer(ByteBuffer.wrap(img.pixels)) }
        .asImageBitmap()
}

@Composable
fun ImageViewer(image: ImageBitmap, name: String, onTap: () -> Unit) {
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    var box by remember { mutableStateOf(IntSize.Zero) }

    fun zoomTo(s: Float, pan: Offset = Offset.Zero) {
        scale = s.coerceIn(1f, 8f)
        val mx = box.width * (scale - 1) / 2
        val my = box.height * (scale - 1) / 2
        offset = (offset + pan).let { Offset(it.x.coerceIn(-mx, mx), it.y.coerceIn(-my, my)) }
    }
    val toggle = { zoomTo(if (scale > 1f) 1f else 2.5f) }
    val zoom = rememberTransformableState { _, z, pan, _ -> zoomTo(scale * z, pan) }

    Image(
        image, name,
        Modifier.fillMaxSize()
            .onSizeChanged { box = it }
            .semantics { onClick(label = if (scale > 1f) "Zoom out" else "Zoom in") { toggle(); true } }
            .pointerInput(Unit) { detectTapGestures(onTap = { onTap() }, onDoubleTap = { toggle() }) }
            .transformable(zoom)
            .graphicsLayer { scaleX = scale; scaleY = scale; translationX = offset.x; translationY = offset.y },
        contentScale = ContentScale.Fit,
    )
}
