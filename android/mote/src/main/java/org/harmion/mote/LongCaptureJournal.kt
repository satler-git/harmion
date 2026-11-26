package org.harmion.mote

import android.content.Context
import org.harmion.sdk.AuthorizationIdentity
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer
import java.nio.CharBuffer
import java.security.DigestInputStream
import java.security.DigestOutputStream
import java.security.MessageDigest

internal data class LongCaptureRecord(
    val id: String,
    val identity: AuthorizationIdentity? = null,
    val body: String = "",
    val draft: String = "",
    val pending: LongCaptureWrite? = null,
) {
    override fun toString(): String = "LongCaptureRecord(redacted)"
}

internal data class LongCaptureWrite(
    val kind: LongCaptureKind,
    val requestJson: String,
    val targetBody: String,
    val receiptId: String? = null,
) {
    override fun toString(): String = "LongCaptureWrite(redacted)"
}

internal enum class LongCaptureKind { Create, Append, Finish }

/** One local long capture and at most one possibly sent write. Callers serialize access. */
internal class LongCaptureJournal(context: Context) {
    private val applicationContext = context.applicationContext
    private val file = DurableJournalFile("mote-long-capture-journal", "long capture journal") {
        applicationContext.noBackupFilesDir
    }

    fun load(): LongCaptureRecord? = file.load(LongCaptureJournalCodec::read)

    // Commit a checksummed tombstone rather than unlinking the previous capture.
    fun write(state: LongCaptureRecord?) = file.write { LongCaptureJournalCodec.write(state, it) }
}

/** Versioned binary envelope; the frozen JSON is opaque and replayed verbatim. */
internal object LongCaptureJournalCodec {
    private const val MAGIC_VERSION = 0x4d4c4301 // MLC + version 1
    private const val MAX_TEXT_BYTES = 8 * 1024 * 1024
    private const val MAX_ID_BYTES = 4096

    fun write(state: LongCaptureRecord?, target: OutputStream) {
        if (state?.pending != null && state.identity == null) malformed()
        val digest = MessageDigest.getInstance("SHA-256")
        val checksummed = DigestOutputStream(target, digest)
        val output = DataOutputStream(checksummed)
        output.writeInt(MAGIC_VERSION)
        output.writeOptional(state) {
            output.writeText(it.id.nonBlank(), MAX_ID_BYTES)
            output.writeOptional(it.identity) { identity ->
                output.writeText(identity.clusterId.nonBlank(), MAX_ID_BYTES)
                output.writeText(identity.nodeId.nonBlank(), MAX_ID_BYTES)
                output.writeText(identity.clientId.nonBlank(), MAX_ID_BYTES)
            }
            output.writeText(it.body, MAX_TEXT_BYTES)
            output.writeText(it.draft, MAX_TEXT_BYTES)
            output.writeOptional(it.pending) { pending ->
                output.writeByte(when (pending.kind) {
                    LongCaptureKind.Create -> 1
                    LongCaptureKind.Append -> 2
                    LongCaptureKind.Finish -> 3
                })
                output.writeText(pending.requestJson.nonBlank(), MAX_TEXT_BYTES)
                output.writeText(pending.targetBody, MAX_TEXT_BYTES)
                output.writeOptional(pending.receiptId) { receiptId ->
                    output.writeText(receiptId.nonBlank(), MAX_ID_BYTES)
                }
            }
        }
        checksummed.on(false)
        output.write(digest.digest())
    }

    fun read(source: InputStream): LongCaptureRecord? {
        val digest = MessageDigest.getInstance("SHA-256")
        val checksummed = DigestInputStream(source, digest)
        val input = DataInputStream(checksummed)
        if (input.readInt() != MAGIC_VERSION) malformed()
        val state = input.readOptional {
            val id = input.readText(MAX_ID_BYTES).nonBlank()
            val identity = input.readOptional {
                AuthorizationIdentity(
                    input.readText(MAX_ID_BYTES).nonBlank(),
                    input.readText(MAX_ID_BYTES).nonBlank(),
                    input.readText(MAX_ID_BYTES).nonBlank(),
                )
            }
            val body = input.readText(MAX_TEXT_BYTES)
            val draft = input.readText(MAX_TEXT_BYTES)
            val pending = input.readOptional {
                if (identity == null) malformed()
                val kind = when (input.readUnsignedByte()) {
                    1 -> LongCaptureKind.Create
                    2 -> LongCaptureKind.Append
                    3 -> LongCaptureKind.Finish
                    else -> malformed()
                }
                LongCaptureWrite(
                    kind,
                    input.readText(MAX_TEXT_BYTES).nonBlank(),
                    input.readText(MAX_TEXT_BYTES),
                    input.readOptional { input.readText(MAX_ID_BYTES).nonBlank() },
                )
            }
            LongCaptureRecord(id, identity, body, draft, pending)
        }
        val expected = digest.digest()
        checksummed.on(false)
        val checksum = ByteArray(expected.size).also(input::readFully)
        if (!MessageDigest.isEqual(checksum, expected) || input.read() != -1) malformed()
        return state
    }

    private inline fun <T : Any> DataOutputStream.writeOptional(value: T?, write: (T) -> Unit) {
        writeByte(if (value == null) 0 else 1)
        if (value != null) write(value)
    }

    private inline fun <T : Any> DataInputStream.readOptional(read: () -> T): T? = when (readUnsignedByte()) {
        0 -> null
        1 -> read()
        else -> malformed()
    }

    private fun DataOutputStream.writeText(value: String, maxBytes: Int) {
        if (value.length > maxBytes) malformed()
        val buffer = Charsets.UTF_8.newEncoder().encode(CharBuffer.wrap(value))
        if (buffer.remaining() > maxBytes) malformed()
        val bytes = ByteArray(buffer.remaining()).also { buffer.get(it) }
        writeInt(bytes.size)
        write(bytes)
    }

    private fun DataInputStream.readText(maxBytes: Int): String {
        val length = readInt()
        if (length !in 0..maxBytes) malformed()
        val bytes = ByteArray(length).also(::readFully)
        return Charsets.UTF_8.newDecoder().decode(ByteBuffer.wrap(bytes)).toString()
    }

    private fun String.nonBlank(): String = if (isBlank()) malformed() else this

    private fun malformed(): Nothing = throw IOException("Malformed long capture journal")
}
