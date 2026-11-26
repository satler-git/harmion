package org.harmion.mote

import org.harmion.sdk.AuthorizationIdentity
import org.harmion.sdk.HarmionException

internal data class LongCaptureView(
    val body: String,
    val open: Boolean?,
    val contested: Boolean,
    val token: String,
) {
    override fun toString(): String = "LongCaptureView(redacted)"
}

internal interface LongCaptureNode {
    val identity: AuthorizationIdentity
    suspend fun readLong(id: String): LongCaptureView
    fun createLongRequest(id: String, body: String): String
    fun appendLongRequest(id: String, token: String, body: String): String
    fun finishLongRequest(id: String, token: String): String
    suspend fun saveLong(request: String): String
}

/** Serial lifecycle for one local session. No write or replay is triggered by loading it. */
internal class LongCaptureSession(
    private val node: LongCaptureNode,
    private val persist: suspend (LongCaptureRecord?) -> Unit,
) {
    suspend fun append(record: LongCaptureRecord) {
        check(record.pending == null && record.draft.isNotBlank())
        checkIdentity(record)
        val target = if (record.body.isEmpty()) record.draft else "${record.body}\n\n${record.draft}"
        val write = if (record.identity == null) {
            LongCaptureWrite(LongCaptureKind.Create, node.createLongRequest(record.id, target), target)
        } else {
            val view = expect(record.id, record.body, open = true)
            LongCaptureWrite(LongCaptureKind.Append, node.appendLongRequest(record.id, view.token, target), target)
        }
        send(record.copy(identity = node.identity, pending = write))
    }

    suspend fun finish(record: LongCaptureRecord) {
        check(record.pending == null && record.draft.isBlank())
        checkIdentity(record)
        if (record.identity == null) {
            persist(null)
            return
        }
        val view = expect(record.id, record.body, open = true)
        val write = LongCaptureWrite(LongCaptureKind.Finish, node.finishLongRequest(record.id, view.token), record.body)
        send(record.copy(pending = write))
    }

    /** Explicit adoption of matching normal values. This never resends a write or invents a receipt. */
    suspend fun checkPending(record: LongCaptureRecord) {
        checkIdentity(record)
        confirm(record, checkNotNull(record.pending))
    }

    private fun checkIdentity(record: LongCaptureRecord) {
        if (record.identity != null && record.identity != node.identity) {
            throw HarmionException("long_authorization_changed", "long capture belongs to another approval")
        }
    }

    private suspend fun send(frozen: LongCaptureRecord) {
        val write = checkNotNull(frozen.pending)
        persist(frozen)
        val receipt = node.saveLong(write.requestJson)
        val accepted = write.copy(receiptId = receipt)
        persist(frozen.copy(pending = accepted))
        // A receipt (including one without a view) says nothing about the currently selected values.
        confirm(frozen, accepted)
    }

    private suspend fun confirm(record: LongCaptureRecord, write: LongCaptureWrite) {
        val stillOpen = write.kind != LongCaptureKind.Finish
        expect(record.id, write.targetBody, stillOpen)
        persist(if (stillOpen) record.copy(body = write.targetBody, draft = "", pending = null) else null)
    }

    private suspend fun expect(id: String, body: String, open: Boolean): LongCaptureView {
        val view = node.readLong(id)
        if (view.contested || view.open != open || view.body != body) {
            throw HarmionException("long_capture_changed", "long capture requires verification")
        }
        return view
    }
}
