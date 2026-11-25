package org.harmion.android.ui

import android.graphics.Bitmap
import android.graphics.Color
import android.util.Base64
import com.google.zxing.BarcodeFormat
import com.google.zxing.qrcode.QRCodeWriter
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.util.zip.GZIPInputStream
import java.util.zip.GZIPOutputStream

/**
 * The peer transfer code format shared with the desktop tools:
 * `harmion-peer:v1:z:<base64url(gzip(PeerRegistration JSON))>`.
 *
 * A code only carries public discovery data. Joining still requires the cluster's
 * membership approval; scanning never grants trust or access by itself.
 */
object PeerCode {
    private const val PREFIX = "harmion-peer:v1:"
    private const val GZIP_PREFIX = "harmion-peer:v1:z:"

    private const val FLAGS = Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP

    fun encode(registrationJson: ByteArray): String {
        val output = ByteArrayOutputStream()
        GZIPOutputStream(output).use { it.write(registrationJson) }
        return GZIP_PREFIX + Base64.encodeToString(output.toByteArray(), FLAGS)
    }

    fun decode(text: String): ByteArray? {
        val trimmed = text.trim()
        return runCatching {
            when {
                trimmed.startsWith(GZIP_PREFIX) ->
                    gunzip(Base64.decode(trimmed.removePrefix(GZIP_PREFIX), FLAGS))
                trimmed.startsWith(PREFIX) ->
                    Base64.decode(trimmed.removePrefix(PREFIX), FLAGS)
                else -> null
            }
        }.getOrNull()
    }

    private fun gunzip(bytes: ByteArray): ByteArray =
        GZIPInputStream(ByteArrayInputStream(bytes)).use { it.readBytes() }
}

object QrCode {
    /** Renders a QR code for display in the management UI. */
    fun render(text: String, size: Int = 768): Bitmap {
        val matrix = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, size, size)
        val pixels = IntArray(size * size)
        for (y in 0 until size) {
            for (x in 0 until size) {
                pixels[y * size + x] = if (matrix[x, y]) Color.BLACK else Color.WHITE
            }
        }
        return Bitmap.createBitmap(pixels, size, size, Bitmap.Config.ARGB_8888)
    }
}
