package org.harmion.mote

import org.harmion.sdk.AuthorizationIdentity
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Test
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.nio.ByteBuffer

class CaptureJournalTest {
    @Test
    fun roundTripsEmptyOrLocalOnlyDraft() {
        val body = "  Draft 😺\r\n改行\u0000 "
        assertEquals(CaptureDraft(), decode(encode(CaptureDraft())))
        assertEquals(CaptureDraft(body), decode(encode(CaptureDraft(body))))
    }

    @Test
    fun roundTripsFrozenRequestAlongsideSeparateDraft() {
        val identity = AuthorizationIdentity(clusterId = "cluster", nodeId = "node", clientId = "client")
        val request = """{ "request_id":"request", "content_id":"content", "text":"first\n一件目" }"""
        val state = CaptureDraft("next draft", PendingCapture(identity, request))

        assertEquals(state, decode(encode(state)))
        assertFalse(state.toString().contains(state.body))
        assertFalse(state.pending.toString().contains(request))
    }

    @Test
    fun rejectsTruncationAndTrailingBytes() {
        val encoded = encode(CaptureDraft("more than empty"))
        for (size in 0 until encoded.size) {
            assertThrows(IOException::class.java) { decode(encoded.copyOf(size)) }
        }
        assertThrows(IOException::class.java) { decode(encoded + 0.toByte()) }
    }

    @Test
    fun rejectsUnsupportedVersionAndOversizedLength() {
        val wrongVersion = encode(CaptureDraft()).also { it[3] = 2 }
        val oversizedBody = encode(CaptureDraft()).also { ByteBuffer.wrap(it).putInt(4, Int.MAX_VALUE) }
        assertThrows(IOException::class.java) { decode(wrongVersion) }
        assertThrows(IOException::class.java) { decode(oversizedBody) }
    }

    @Test
    fun rejectsCorruptionThatStillDecodesAsText() {
        val changedBody = encode(CaptureDraft("body")).also { it[8] = 'B'.code.toByte() }
        assertThrows(IOException::class.java) { decode(changedBody) }
    }

    @Test
    fun refusesToWriteIncompletePendingCapture() {
        val identity = AuthorizationIdentity(clusterId = "cluster", nodeId = "node", clientId = "client")
        assertThrows(IOException::class.java) {
            encode(CaptureDraft(pending = PendingCapture(identity.copy(clientId = ""), "{}")))
        }
        assertThrows(IOException::class.java) {
            encode(CaptureDraft(pending = PendingCapture(identity, " ")))
        }
    }

    private fun encode(state: CaptureDraft): ByteArray =
        ByteArrayOutputStream().also { CaptureJournalCodec.write(state, it) }.toByteArray()

    private fun decode(bytes: ByteArray): CaptureDraft = CaptureJournalCodec.read(bytes.inputStream())
}
