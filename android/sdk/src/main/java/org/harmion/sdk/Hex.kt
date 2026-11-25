package org.harmion.sdk

/** Hex helpers shared by the SDK and the node app. */
object Hex {
    fun encode(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it) }

    fun decode(text: String): ByteArray {
        require(text.length % 2 == 0) { "hex length must be even" }
        return ByteArray(text.length / 2) { index ->
            text.substring(index * 2, index * 2 + 2).toInt(16).toByte()
        }
    }

    fun decodeOrNull(text: String): ByteArray? =
        runCatching { decode(text) }.getOrNull()
}
