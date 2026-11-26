package org.harmion.mote

import android.content.Context
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import org.harmion.sdk.AuthorizationIdentity
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer
import java.nio.CharBuffer
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.NoSuchFileException
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.security.DigestInputStream
import java.security.DigestOutputStream
import java.security.MessageDigest

internal data class CaptureDraft(val body: String = "", val pending: PendingCapture? = null) {
    override fun toString(): String = "CaptureDraft(redacted)"
}

internal data class PendingCapture(val identity: AuthorizationIdentity, val requestJson: String) {
    override fun toString(): String = "PendingCapture(redacted)"
}

/** One local draft and at most one possibly sent create. Callers serialize all access. */
internal class CaptureJournal(context: Context, shared: Boolean = false) {
    private val applicationContext = context.applicationContext
    private val file = DurableJournalFile(if (shared) "mote-share-journal" else "mote-capture-journal", "capture journal") {
        applicationContext.noBackupFilesDir
    }

    fun load(): CaptureDraft = file.load(CaptureJournalCodec::read) ?: CaptureDraft()

    fun write(state: CaptureDraft) = file.write { CaptureJournalCodec.write(state, it) }
}

/** Shared filesystem boundary for journal codecs. Callers serialize access to each file. */
internal class DurableJournalFile(
    private val fileName: String,
    private val label: String,
    private val directory: () -> File?,
) {
    fun <T> load(read: (InputStream) -> T): T? = withDirectory("read") { directory ->
        val path = directory.resolve(fileName)
        val input = try {
            Files.newInputStream(path)
        } catch (error: NoSuchFileException) {
            // A staging file is uncommitted, even on the first write. Do not mistake a
            // missing directory or a dangling journal entry for a new installation.
            if (Files.isDirectory(directory) && Files.notExists(path, LinkOption.NOFOLLOW_LINKS)) {
                return@withDirectory null
            }
            throw error
        }
        input.buffered().use(read)
    }

    fun write(write: (OutputStream) -> Unit): Unit = withDirectory("write") { directory ->
        val staging = directory.resolve("$fileName.new")
        val path = directory.resolve(fileName)
        // AtomicFile.finishWrite can log sync/rename failures instead of reporting them.
        // Only allow a sender to proceed after both file contents and replacement are synced.
        FileOutputStream(staging.toFile()).use { file ->
            val buffer = file.buffered()
            write(buffer)
            buffer.flush()
            file.fd.sync()
        }
        Files.move(staging, path, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        val directoryFd = Os.open(directory.toString(), OsConstants.O_RDONLY, 0)
        try {
            Os.fsync(directoryFd)
        } finally {
            Os.close(directoryFd)
        }
    }

    private inline fun <T> withDirectory(operation: String, block: (Path) -> T): T {
        try {
            val directory = directory() ?: throw IOException()
            return block(directory.toPath())
        } catch (_: IOException) {
            throw IOException("Could not $operation $label")
        } catch (_: ErrnoException) {
            throw IOException("Could not $operation $label")
        } catch (_: SecurityException) {
            throw IOException("Could not $operation $label")
        }
    }
}

/** Private, versioned binary envelope; request JSON is sealed and replayed verbatim. */
internal object CaptureJournalCodec {
    private const val MAGIC_VERSION = 0x4d4f5401 // MOT + version 1
    // Bound reads of corrupt lengths. The local draft can exceed the network request limit.
    private const val MAX_TEXT_BYTES = 8 * 1024 * 1024
    private const val MAX_ID_BYTES = 4096

    fun write(state: CaptureDraft, target: OutputStream) {
        val digest = MessageDigest.getInstance("SHA-256")
        val checksummed = DigestOutputStream(target, digest)
        val output = DataOutputStream(checksummed)
        output.writeInt(MAGIC_VERSION)
        output.writeText(state.body, MAX_TEXT_BYTES)
        val pending = state.pending
        output.writeByte(if (pending == null) 0 else 1)
        if (pending != null) {
            output.writeText(pending.identity.clusterId.nonBlank(), MAX_ID_BYTES)
            output.writeText(pending.identity.nodeId.nonBlank(), MAX_ID_BYTES)
            output.writeText(pending.identity.clientId.nonBlank(), MAX_ID_BYTES)
            output.writeText(pending.requestJson.nonBlank(), MAX_TEXT_BYTES)
        }
        checksummed.on(false)
        output.write(digest.digest())
    }

    fun read(source: InputStream): CaptureDraft {
        val digest = MessageDigest.getInstance("SHA-256")
        val checksummed = DigestInputStream(source, digest)
        val input = DataInputStream(checksummed)
        if (input.readInt() != MAGIC_VERSION) malformed()
        val body = input.readText(MAX_TEXT_BYTES)
        val pending = when (input.readUnsignedByte()) {
            0 -> null
            1 -> PendingCapture(
                AuthorizationIdentity(
                    input.readText(MAX_ID_BYTES).nonBlank(),
                    input.readText(MAX_ID_BYTES).nonBlank(),
                    input.readText(MAX_ID_BYTES).nonBlank(),
                ),
                input.readText(MAX_TEXT_BYTES).nonBlank(),
            )
            else -> malformed()
        }
        val expected = digest.digest()
        checksummed.on(false)
        val checksum = ByteArray(expected.size).also(input::readFully)
        if (!MessageDigest.isEqual(checksum, expected) || input.read() != -1) malformed()
        return CaptureDraft(body, pending)
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

    private fun malformed(): Nothing = throw IOException("Malformed capture journal")
}
