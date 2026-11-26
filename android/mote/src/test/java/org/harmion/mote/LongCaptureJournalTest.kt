package org.harmion.mote

import org.harmion.sdk.AuthorizationIdentity
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.nio.ByteBuffer

class LongCaptureJournalTest {
    @get:Rule val folder = TemporaryFolder()

    private val identity = AuthorizationIdentity("cluster-id", "node-id", "client-id")
    private val frozenJson = " \r\n{ \"text\" : \"一件目\\n😺\", \"request_id\": \"write-id\" }\n "

    @Test
    fun roundTripsLocalDraftAndEstablishedCapture() {
        val local = LongCaptureRecord("local-id", draft = "  new draft\r\n😺\u0000 ")
        val established = local.copy(identity = identity, body = "previous body\n一件目")
        assertEquals(LongCaptureRecord("local-id"), decode(encode(LongCaptureRecord("local-id"))))
        assertEquals(local, decode(encode(local)))
        assertEquals(established, decode(encode(established)))
    }

    @Test
    fun roundTripsEachPendingKindAndItsReceipt() {
        for (kind in LongCaptureKind.entries) {
            val pending = LongCaptureWrite(kind, frozenJson, "target body\n一件目")
            val record = LongCaptureRecord("capture-id", identity, "previous body", "current draft", pending)
            assertEquals(record, decode(encode(record)))

            val withReceipt = record.copy(pending = pending.copy(receiptId = "receipt-id"))
            assertEquals(withReceipt, decode(encode(withReceipt)))
        }
    }

    @Test
    fun missingCommittedFileAndTombstoneBothLoadAsNoSession() {
        val path = folder.root.resolve("journal")
        val file = DurableJournalFile(path.name, "long capture journal") { folder.root }
        assertNull(file.load(LongCaptureJournalCodec::read))

        val record = LongCaptureRecord("local-id", draft = "local draft")
        folder.root.resolve("${path.name}.new").writeBytes(encode(record))
        assertNull(file.load(LongCaptureJournalCodec::read))

        path.writeBytes(encode(record))
        assertEquals(record, file.load(LongCaptureJournalCodec::read))
        path.writeBytes(encode(null))
        assertNull(file.load(LongCaptureJournalCodec::read))
    }

    @Test
    fun rejectsTruncationAndTrailingBytesOfRecordAndTombstone() {
        val pending = LongCaptureWrite(LongCaptureKind.Finish, frozenJson, "target body", "receipt-id")
        val record = LongCaptureRecord("capture-id", identity, "body", "draft", pending)
        for (state in listOf(record, null)) {
            val encoded = encode(state)
            for (size in 0 until encoded.size) {
                assertThrows(IOException::class.java) { decode(encoded.copyOf(size)) }
            }
            assertThrows(IOException::class.java) { decode(encoded + 0.toByte()) }
        }
    }

    @Test
    fun rejectsUnsupportedVersionOversizedLengthAndReadableCorruption() {
        val encoded = encode(LongCaptureRecord("capture-id"))
        val wrongVersion = encoded.copyOf().also { it[3] = 2 }
        val oversizedId = encoded.copyOf().also { ByteBuffer.wrap(it).putInt(5, Int.MAX_VALUE) }
        val changedId = encoded.copyOf().also { it[9] = 'C'.code.toByte() }
        for (bytes in listOf(wrongVersion, oversizedId, changedId)) {
            assertThrows(IOException::class.java) { decode(bytes) }
        }
    }

    @Test
    fun refusesToPersistPendingWriteWithoutIdentity() {
        for (kind in LongCaptureKind.entries) {
            val pending = LongCaptureWrite(kind, frozenJson, "target body")
            assertThrows(IOException::class.java) { encode(LongCaptureRecord("capture-id", pending = pending)) }
        }
    }

    @Test
    fun redactsRecordedContentAndReadErrors() {
        val pending = LongCaptureWrite(LongCaptureKind.Append, frozenJson, "target body", "receipt-id")
        val record = LongCaptureRecord("capture-id", identity, "previous body", "local draft", pending)
        assertEquals("LongCaptureRecord(redacted)", record.toString())
        assertEquals("LongCaptureWrite(redacted)", pending.toString())

        val path = folder.root.resolve("private-path")
        val file = DurableJournalFile(path.name, "long capture journal") { folder.root }
        path.writeBytes(encode(record).dropLast(1).toByteArray())
        val error = assertThrows(IOException::class.java) { file.load(LongCaptureJournalCodec::read) }
        assertEquals("Could not read long capture journal", error.message)
        assertNull(error.cause)
        assertFalse(error.toString().contains(path.name))
    }

    private fun encode(state: LongCaptureRecord?): ByteArray =
        ByteArrayOutputStream().also { LongCaptureJournalCodec.write(state, it) }.toByteArray()

    private fun decode(bytes: ByteArray): LongCaptureRecord? = LongCaptureJournalCodec.read(bytes.inputStream())
}
