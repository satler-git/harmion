package org.harmion.mote

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import org.harmion.sdk.AuthorizationIdentity
import org.harmion.sdk.HarmionException
import org.harmion.sdk.HarmionSession
import org.harmion.sdk.NodeStatus
import org.harmion.sdk.Requests
import org.harmion.sdk.SubscriptionEvent
import org.json.JSONObject

internal const val CAPTURE_KIND = "capture"
private const val CAPTURE_NAMESPACE = "capture"
private const val CREATED_AT = "created_at_micros"
private const val ARCHIVED = "archived"
private const val PINNED = "pinned"
private const val SESSION_OPEN = "session_open"
private const val REPLY_TO = "reply_to"

internal sealed interface CaptureSnapshot {
    data object Loading : CaptureSnapshot
    data class Ready(val items: List<Capture>) : CaptureSnapshot
}

/** The node is authoritative; only the local draft and its frozen create request live in Mote. */
internal class CaptureRepository(
    private val session: HarmionSession,
    override val identity: AuthorizationIdentity,
) : LongCaptureNode {
    fun observe(): Flow<CaptureSnapshot> = captureSnapshots(session.observe(Requests.kindIs(CAPTURE_KIND), identity))

    fun prepareCreate(body: String, replyTo: String? = null): PendingCapture {
        require(body.isNotBlank())
        return PendingCapture(identity, createRequest(Requests.randomIdHex(), body, open = false, replyTo = replyTo))
    }

    /** A pause-free nudge for the node scheduler; the node still applies its own policy. */
    suspend fun requestSync() = session.requestSync()

    suspend fun syncStatus(): NodeStatus =
        kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { session.status() }

    /** Always reuse the exact payload and authorization identity, even on manual retry. */
    suspend fun create(pending: PendingCapture) {
        if (pending.identity != identity) {
            throw HarmionException("pending_authorization_changed", "pending capture belongs to another approval")
        }
        // A null view is allowed on success. The receipt, not the view or peer sync, confirms the save.
        val result = session.save(pending.requestJson, pending.identity)
        check(result.updateId.isNotBlank()) { "missing save receipt" }
    }

    private fun createRequest(id: String, body: String, open: Boolean, replyTo: String? = null): String = boundedRequest(Requests.create(
        id,
        buildList {
            add(Requests.body(body))
            add(Requests.kind(CAPTURE_KIND))
            add(Requests.attribute(CAPTURE_NAMESPACE, CREATED_AT, System.currentTimeMillis() * 1000L))
            if (open) add(Requests.attribute(CAPTURE_NAMESPACE, SESSION_OPEN, true))
            if (replyTo != null) add(Requests.attribute(CAPTURE_NAMESPACE, REPLY_TO, replyTo))
        },
    ))

    override fun createLongRequest(id: String, body: String): String = createRequest(id, body, open = true)

    override fun appendLongRequest(id: String, token: String, body: String): String =
        boundedRequest(Requests.edit(id, token, listOf(Requests.body(body))))

    override fun finishLongRequest(id: String, token: String): String =
        Requests.edit(id, token, listOf(Requests.attribute(CAPTURE_NAMESPACE, SESSION_OPEN, false)))

    override suspend fun saveLong(request: String): String = session.save(request, identity).updateId

    override suspend fun readLong(id: String): LongCaptureView = currentView(id).longCapture()

    suspend fun setArchived(capture: Capture, value: Boolean) = patch(capture, ARCHIVED, capture.archived, value)

    suspend fun setPinned(capture: Capture, value: Boolean) = patch(capture, PINNED, capture.pinned, value)

    private suspend fun patch(capture: Capture, key: String, expected: Boolean, value: Boolean) {
        val current = currentView(capture.id)
        if (current.hasDeletionConflict || current.hasAttributeConflict(key) ||
            current.attributeFlag(key) != expected
        ) changed()
        if (expected == value) return
        session.save(
            Requests.edit(capture.id, current.token, listOf(Requests.attribute(CAPTURE_NAMESPACE, key, value))),
            identity,
        )
    }

    suspend fun delete(capture: Capture) {
        val current = currentView(capture.id)
        if (current.attributeFlag(SESSION_OPEN) || current.hasAttributeConflict(SESSION_OPEN)) {
            throw HarmionException("capture_session_open", "open capture session cannot be deleted")
        }
        // Delete only the confirmed record. Do not automatically delete multi-Kind or derived data.
        if (current.kinds != setOf(CAPTURE_KIND)) {
            throw HarmionException("capture_shared", "capture has other kinds")
        }
        if (current.hasDeletionConflict || current.hasConflicts ||
            current.capture().copy(token = capture.token) != capture
        ) changed()
        // A fresh get verifies visibility and the displayed values, but is not a CAS. Keep the
        // confirmed basis; never silently substitute a newer token in a destructive operation.
        session.save(Requests.delete(capture.id, capture.token), identity)
    }

    /** Closes an open draft session and deletes its Content, with the displayed values checked first. */
    suspend fun deleteDraftSession(capture: Capture) = deleteSession(capture.id, capture)

    /** The local journal owns this draft, so there is no snapshot projection to compare with. */
    suspend fun deleteDraftSession(id: String) = deleteSession(id, null)

    private suspend fun deleteSession(id: String, expected: Capture?) {
        val current = currentView(id)
        if (current.kinds != setOf(CAPTURE_KIND)) {
            throw HarmionException("capture_shared", "capture has other kinds")
        }
        if (current.hasDeletionConflict || current.hasConflicts) changed()
        if (expected != null && current.capture().copy(token = expected.token) != expected) changed()
        if (!current.attributeFlag(SESSION_OPEN)) {
            // Already closed; delete with the freshly read token.
            if (expected == null && current.hasAttributeConflict(SESSION_OPEN)) changed()
            session.save(Requests.delete(id, current.token), identity)
            return
        }
        if (current.hasAttributeConflict(SESSION_OPEN)) changed()
        val body = current.capture().body
        session.save(
            Requests.edit(id, current.token, listOf(Requests.attribute(CAPTURE_NAMESPACE, SESSION_OPEN, false))),
            identity,
        )
        val closed = currentView(id)
        if (closed.attributeFlag(SESSION_OPEN) || closed.hasDeletionConflict || closed.hasConflicts ||
            closed.kinds != setOf(CAPTURE_KIND) || closed.capture().body != body
        ) changed()
        session.save(Requests.delete(id, closed.token), identity)
    }

    suspend fun editBody(capture: Capture, body: String) {
        require(body.isNotBlank())
        val current = currentView(capture.id)
        val latest = current.capture()
        if (latest.isLong || current.hasDeletionConflict || current.hasConflicts ||
            latest.body != capture.body || latest.archived != capture.archived ||
            latest.pinned != capture.pinned || latest.sessionOpen != capture.sessionOpen ||
            latest.replyTo != capture.replyTo
        ) changed()
        if (latest.body == body) return
        val request = boundedRequest(Requests.edit(capture.id, current.token, listOf(Requests.body(body))))
        session.save(request, identity)
    }

    private suspend fun currentView(id: String): CaptureReadView = CaptureReadView(session.get(id, identity), id)
}

// Bound the encoded request before it is journaled; the node enforces its own binary limits.
private fun boundedRequest(request: String): String {
    if (request.toByteArray(Charsets.UTF_8).size > 1024 * 1024) {
        throw HarmionException("capture_too_large", "encoded capture is too large")
    }
    return request
}

private fun changed(): Nothing = throw HarmionException("capture_changed", "capture changed before the operation")

internal fun captureSnapshots(events: Flow<SubscriptionEvent>): Flow<CaptureSnapshot> = flow {
    var index: Map<String, Capture>? = null
    var resets = 0
    events.collect { event ->
        when (event) {
            SubscriptionEvent.SnapshotChunk, SubscriptionEvent.Idle -> Unit
            is SubscriptionEvent.SnapshotEnd -> {
                index = event.contents.mapValues { (id, view) -> CaptureReadView(view, id).capture() }
                emit(CaptureSnapshot.Ready(checkNotNull(index).values.toList()))
            }
            is SubscriptionEvent.Changes -> {
                val previous = index ?: invalidSnapshot()
                val updated = previous.toMutableMap()
                event.removed.forEach(updated::remove)
                for (id in event.upserted) {
                    val view = event.contents[id] ?: invalidSnapshot()
                    updated[id] = CaptureReadView(view, id).capture()
                }
                index = updated
                emit(CaptureSnapshot.Ready(updated.values.toList()))
            }
            SubscriptionEvent.ResetRequired -> {
                index = null
                emit(CaptureSnapshot.Loading)
                if (++resets > 3) throw HarmionException("capture_reset_limit", "too many subscription resets")
            }
        }
    }
}

private fun invalidSnapshot(): Nothing = throw HarmionException("invalid_response", "incomplete capture view")

// Harmion's canonical decimal serializer may emit an integer using scientific notation.
// Android's JSONObject reads that form as Double; accept only exact integral microseconds.
internal fun captureTimestamp(value: Any?): Long? {
    if (value is Long) return value
    if (value is Int) return value.toLong()
    if (value !is Double || !value.isFinite() || kotlin.math.abs(value) >= 9_007_199_254_740_992.0) return null
    val integer = value.toLong()
    return integer.takeIf { it.toDouble() == value }
}

private class CaptureReadView(view: JSONObject, expectedId: String) {
    private val content = view.getJSONObject("content")
    private val id = (content.get("id") as? String)?.takeIf { it == expectedId } ?: invalidSnapshot()
    val token = (view.get("edit_token") as? String)?.takeIf { it.isNotEmpty() } ?: invalidSnapshot()
    val kinds: Set<String> = content.getJSONArray("kinds").let { array ->
        (0 until array.length()).map { array.get(it) as? String ?: invalidSnapshot() }.toSet()
    }
    private val attributes = content.getJSONArray("attributes").let { array ->
        buildMap {
            for (i in 0 until array.length()) {
                val entry = array.getJSONObject(i)
                if (entry.getString("namespace") == CAPTURE_NAMESPACE) {
                    put(entry.getString("key"), entry.get("value"))
                }
            }
        }
    }
    private val conflicts = view.getJSONArray("conflicts")
    val hasConflicts: Boolean get() = conflicts.length() > 0
    val hasDeletionConflict: Boolean = view.get("deletion_conflict") as? Boolean ?: invalidSnapshot()

    init {
        if (CAPTURE_KIND !in kinds) invalidSnapshot()
    }

    fun hasAttributeConflict(key: String): Boolean = (0 until conflicts.length()).any { index ->
        val field = conflicts.getJSONObject(index)
        field.optString("type") == "attribute" && field.optString("namespace") == CAPTURE_NAMESPACE &&
            field.optString("key") == key
    }

    fun attributeFlag(key: String): Boolean = attributes[key] == true

    private fun body(): String = content.get("body") as? String ?: invalidSnapshot()

    fun longCapture(): LongCaptureView = LongCaptureView(
        body = body(),
        open = attributes[SESSION_OPEN] as? Boolean,
        contested = hasDeletionConflict || kinds != setOf(CAPTURE_KIND) ||
            (0 until conflicts.length()).any { index ->
                val field = conflicts.getJSONObject(index)
                when (field.getString("type")) {
                    "body", "kind" -> true
                    "attribute" -> field.optString("namespace") == CAPTURE_NAMESPACE &&
                        field.optString("key") == SESSION_OPEN
                    else -> true
                }
            },
        token = token,
    )

    fun capture(): Capture = Capture(
        id = id,
        body = body(),
        createdAtMicros = captureTimestamp(attributes[CREATED_AT]),
        archived = attributeFlag(ARCHIVED),
        pinned = attributeFlag(PINNED),
        token = token,
        sessionOpen = attributeFlag(SESSION_OPEN),
        isLong = attributes[SESSION_OPEN] is Boolean,
        replyTo = (attributes[REPLY_TO] as? String)?.takeIf { it.isNotBlank() },
    )
}
