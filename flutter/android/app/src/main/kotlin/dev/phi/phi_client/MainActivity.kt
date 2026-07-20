package dev.phi.phi_client

import android.app.Activity
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import android.provider.OpenableColumns
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import java.io.ByteArrayOutputStream
import kotlin.math.max
import kotlin.math.roundToInt

class MainActivity : FlutterActivity() {
    companion object {
        private const val CHANNEL = "dev.phi.phi_client/image_attachment_picker"
        private const val PICK_IMAGES_REQUEST = 7301
        private const val MAX_IMAGES = 3
        private const val MAX_DECODE_DIMENSION = 2048
        private const val MAX_OUTPUT_DIMENSION = 1600
        private const val MAX_IMAGE_BYTES = 200 * 1024
    }

    private var pendingPickerResult: MethodChannel.Result? = null
    private var pendingMaxCount = MAX_IMAGES

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL)
            .setMethodCallHandler { call, result ->
                if (call.method != "pickImages") {
                    result.notImplemented()
                    return@setMethodCallHandler
                }
                if (pendingPickerResult != null) {
                    result.error("picker_active", "The image picker is already open.", null)
                    return@setMethodCallHandler
                }

                pendingMaxCount =
                    (call.argument<Int>("maxCount") ?: MAX_IMAGES).coerceIn(1, MAX_IMAGES)
                pendingPickerResult = result
                val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                    addCategory(Intent.CATEGORY_OPENABLE)
                    type = "image/*"
                    putExtra(Intent.EXTRA_ALLOW_MULTIPLE, pendingMaxCount > 1)
                }
                try {
                    startActivityForResult(intent, PICK_IMAGES_REQUEST)
                } catch (error: Exception) {
                    pendingPickerResult = null
                    result.error("picker_unavailable", error.message, null)
                }
            }
    }

    @Deprecated("Deprecated in Android; retained for FlutterActivity compatibility.")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode != PICK_IMAGES_REQUEST) {
            super.onActivityResult(requestCode, resultCode, data)
            return
        }

        val channelResult = pendingPickerResult ?: return
        pendingPickerResult = null
        if (resultCode != Activity.RESULT_OK || data == null) {
            channelResult.success(emptyList<Map<String, Any>>())
            return
        }

        val uris = selectedUris(data).take(pendingMaxCount)
        Thread {
            try {
                val images = uris.map(::prepareImage)
                runOnUiThread { channelResult.success(images) }
            } catch (error: Exception) {
                runOnUiThread {
                    channelResult.error("image_processing_failed", error.message, null)
                }
            }
        }.start()
    }

    private fun selectedUris(data: Intent): List<Uri> {
        val clip = data.clipData
        if (clip != null) {
            return List(clip.itemCount) { index -> clip.getItemAt(index).uri }
        }
        return listOfNotNull(data.data)
    }

    private fun prepareImage(uri: Uri): Map<String, Any> {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        contentResolver.openInputStream(uri).use { input ->
            requireNotNull(input) { "Cannot open the selected image." }
            BitmapFactory.decodeStream(input, null, bounds)
        }
        require(bounds.outWidth > 0 && bounds.outHeight > 0) {
            "The selected file is not a readable image."
        }

        var sampleSize = 1
        while (max(bounds.outWidth, bounds.outHeight) / sampleSize > MAX_DECODE_DIMENSION) {
            sampleSize *= 2
        }
        val options = BitmapFactory.Options().apply {
            inSampleSize = sampleSize
            inPreferredConfig = Bitmap.Config.ARGB_8888
        }
        var bitmap = contentResolver.openInputStream(uri).use { input ->
            requireNotNull(input) { "Cannot open the selected image." }
            requireNotNull(BitmapFactory.decodeStream(input, null, options)) {
                "The selected image could not be decoded."
            }
        }

        bitmap = orientBitmap(bitmap, uri)
        val longestSide = max(bitmap.width, bitmap.height)
        if (longestSide > MAX_OUTPUT_DIMENSION) {
            val scale = MAX_OUTPUT_DIMENSION.toFloat() / longestSide
            val scaled = Bitmap.createScaledBitmap(
                bitmap,
                (bitmap.width * scale).roundToInt().coerceAtLeast(1),
                (bitmap.height * scale).roundToInt().coerceAtLeast(1),
                true,
            )
            if (scaled !== bitmap) bitmap.recycle()
            bitmap = scaled
        }

        val opaque = Bitmap.createBitmap(bitmap.width, bitmap.height, Bitmap.Config.ARGB_8888)
        Canvas(opaque).apply {
            drawColor(Color.WHITE)
            drawBitmap(bitmap, 0f, 0f, null)
        }
        if (opaque !== bitmap) bitmap.recycle()

        val bytes = try {
            compressToLimit(opaque)
        } finally {
            opaque.recycle()
        }
        val baseName = displayName(uri).substringBeforeLast('.').ifBlank { "image" }
        return mapOf(
            "name" to "$baseName.jpg",
            "mimeType" to "image/jpeg",
            "bytes" to bytes,
        )
    }

    private fun orientBitmap(source: Bitmap, uri: Uri): Bitmap {
        val orientation = try {
            contentResolver.openInputStream(uri).use { input ->
                if (input == null) ExifInterface.ORIENTATION_NORMAL
                else ExifInterface(input).getAttributeInt(
                    ExifInterface.TAG_ORIENTATION,
                    ExifInterface.ORIENTATION_NORMAL,
                )
            }
        } catch (_: Exception) {
            ExifInterface.ORIENTATION_NORMAL
        }
        val matrix = Matrix().apply {
            when (orientation) {
                ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> setScale(-1f, 1f)
                ExifInterface.ORIENTATION_ROTATE_180 -> setRotate(180f)
                ExifInterface.ORIENTATION_FLIP_VERTICAL -> setScale(1f, -1f)
                ExifInterface.ORIENTATION_TRANSPOSE -> {
                    setRotate(90f)
                    postScale(-1f, 1f)
                }
                ExifInterface.ORIENTATION_ROTATE_90 -> setRotate(90f)
                ExifInterface.ORIENTATION_TRANSVERSE -> {
                    setRotate(-90f)
                    postScale(-1f, 1f)
                }
                ExifInterface.ORIENTATION_ROTATE_270 -> setRotate(-90f)
            }
        }
        if (matrix.isIdentity) return source
        val rotated = Bitmap.createBitmap(
            source,
            0,
            0,
            source.width,
            source.height,
            matrix,
            true,
        )
        if (rotated !== source) source.recycle()
        return rotated
    }

    private fun compressToLimit(source: Bitmap): ByteArray {
        var working = source
        var ownsWorking = false
        var quality = 88
        try {
            while (true) {
                val output = ByteArrayOutputStream()
                working.compress(Bitmap.CompressFormat.JPEG, quality, output)
                val bytes = output.toByteArray()
                if (bytes.size <= MAX_IMAGE_BYTES) return bytes

                if (quality > 48) {
                    quality -= 8
                    continue
                }
                val width = (working.width * 0.82).roundToInt().coerceAtLeast(1)
                val height = (working.height * 0.82).roundToInt().coerceAtLeast(1)
                require(max(width, height) >= 480) {
                    "The selected image cannot be compressed enough to send."
                }
                val scaled = Bitmap.createScaledBitmap(working, width, height, true)
                if (ownsWorking && scaled !== working) working.recycle()
                working = scaled
                ownsWorking = working !== source
                quality = 80
            }
        } finally {
            if (ownsWorking) working.recycle()
        }
    }

    private fun displayName(uri: Uri): String {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    if (index >= 0) return cursor.getString(index)
                }
            }
        return uri.lastPathSegment ?: "image"
    }
}
