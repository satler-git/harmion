package org.harmion.mote

import android.content.Context
import java.io.IOException
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

internal data class CachedCaptures(val savedAtMicros: Long, val items: List<Capture>) {
    override fun toString(): String = "CachedCaptures(count=${items.size})"
}

/**
 * Best-effort, app-private copy of the last complete snapshot. It is only
 * displayed while the node is being reached; it never authorizes a mutation and
 * deliberately does not persist edit tokens. Backups are excluded.
 */
internal class CaptureCache(context: Context) {
    private val file = DurableJournalFile("mote-capture-cache", "capture cache") {
        context.applicationContext.noBackupFilesDir
    }

    fun load(): CachedCaptures? = file.load { input ->
        CaptureCacheCodec.decode(input.readBytes())
    }

    fun write(items: List<Capture>) = file.write { output ->
        output.write(CaptureCacheCodec.encode(items))
    }
}

internal object CaptureCacheCodec {
    private const val VERSION = 1
    private const val MAX_BYTES = 16 * 1024 * 1024

    fun encode(items: List<Capture>): ByteArray {
        val captures = JSONArray()
        for (capture in items) {
            captures.put(
                JSONObject()
                    .put("id", capture.id)
                    .put("body", capture.body)
                    .put("created_at", capture.createdAtMicros ?: JSONObject.NULL)
                    .put("archived", capture.archived)
                    .put("pinned", capture.pinned)
                    .put("session_open", capture.sessionOpen)
                    .put("is_long", capture.isLong)
                    .apply { if (capture.replyTo != null) put("reply_to", capture.replyTo) },
            )
        }
        val root = JSONObject()
            .put("version", VERSION)
            .put("saved_at_micros", System.currentTimeMillis() * 1_000L)
            .put("captures", captures)
        val bytes = root.toString().toByteArray(Charsets.UTF_8)
        if (bytes.size > MAX_BYTES) throw IOException("capture cache is too large")
        return bytes
    }

    fun decode(bytes: ByteArray): CachedCaptures {
        try {
            val root = JSONObject(String(bytes, Charsets.UTF_8))
            if (root.optInt("version", -1) != VERSION) throw IOException("unsupported capture cache")
            val array = root.optJSONArray("captures") ?: throw IOException("missing captures")
            val items = (0 until array.length()).map { index ->
                val entry = array.getJSONObject(index)
                Capture(
                    id = entry.getString("id"),
                    body = entry.getString("body"),
                    createdAtMicros =
                        if (entry.has("created_at") && !entry.isNull("created_at")) entry.getLong("created_at") else null,
                    archived = entry.optBoolean("archived"),
                    pinned = entry.optBoolean("pinned"),
                    // A cached row is read-only; never carry a node edit token across processes.
                    token = "",
                    sessionOpen = entry.optBoolean("session_open"),
                    isLong = entry.optBoolean("is_long"),
                    replyTo =
                        if (entry.has("reply_to") && !entry.isNull("reply_to")) {
                            entry.getString("reply_to").ifEmpty { null }
                        } else {
                            null
                        },
                )
            }
            val savedAt = root.optLong("saved_at_micros", 0L)
            return CachedCaptures(savedAt, items)
        } catch (error: JSONException) {
            throw IOException("malformed capture cache", error)
        }
    }
}
