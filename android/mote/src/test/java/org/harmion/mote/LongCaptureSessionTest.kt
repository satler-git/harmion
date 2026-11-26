package org.harmion.mote

import kotlinx.coroutines.runBlocking
import org.harmion.sdk.AuthorizationIdentity
import org.harmion.sdk.HarmionException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class LongCaptureSessionTest {
    @Test
    fun createsAppendsAndClosesTheSameContent() = runBlocking {
        val fixture = Fixture()
        fixture.node.next = matching("最初")
        fixture.session.append(LongCaptureRecord("same-id", draft = "最初"))
        assertEquals(listOf("create:same-id"), fixture.node.sent)
        assertEquals("最初", fixture.record?.body)
        fixture.node.next = matching("最初\n\n次の入力")
        fixture.session.append(checkNotNull(fixture.record).copy(draft = "次の入力"))
        assertEquals("最初\n\n次の入力", fixture.record?.body)
        fixture.node.next = matching("最初\n\n次の入力", open = false)
        fixture.session.finish(checkNotNull(fixture.record))
        assertEquals(listOf("create:same-id", "append:same-id:basis", "finish:same-id:basis"), fixture.node.sent)
        assertNull(fixture.record)
    }

    @Test
    fun emptySessionClosesWithoutCreatingContent() = runBlocking {
        val fixture = Fixture()
        fixture.session.finish(LongCaptureRecord("only-local"))
        assertTrue(fixture.node.sent.isEmpty())
        assertNull(fixture.record)
    }

    @Test
    fun responseLossKeepsTheDraftAndExplicitCheckNeverResends() {
        val fixture = Fixture()
        val original = fixture.open("記録済み", "追記")
        fixture.node.view = matching(original.body)
        fixture.node.next = matching("記録済み\n\n追記")
        fixture.node.loseReceipt = true
        assertThrows(HarmionException::class.java) { runBlocking { fixture.session.append(original) } }
        val pending = checkNotNull(fixture.record)
        assertEquals("追記", pending.draft)
        assertEquals(LongCaptureKind.Append, pending.pending?.kind)
        assertNull(pending.pending?.receiptId)
        runBlocking { fixture.session.checkPending(pending) }
        assertEquals(1, fixture.node.sent.size)
        assertEquals("記録済み\n\n追記", fixture.record?.body)
        assertNull(fixture.record?.pending)
    }

    @Test
    fun acceptedCloseStaysPendingWhenBodyChangedAndMatchingCheckCannotHideAConflict() {
        val fixture = Fixture()
        val original = fixture.open("本文")
        fixture.node.view = matching(original.body)
        fixture.node.next = matching("外部で変更", open = false)
        val closeError = assertThrows(HarmionException::class.java) { runBlocking { fixture.session.finish(original) } }
        assertEquals("long_capture_changed", closeError.code)
        val pending = checkNotNull(fixture.record)
        assertEquals("receipt", pending.pending?.receiptId)
        fixture.node.view = matching(original.body, open = false).copy(contested = true)
        assertThrows(HarmionException::class.java) { runBlocking { fixture.session.checkPending(pending) } }
        assertEquals(pending, fixture.record)
        assertEquals(1, fixture.node.sent.size)
    }

    @Test
    fun foreignAuthorizationOrPreflightMismatchNeverWrites() {
        val fixture = Fixture()
        val original = fixture.open("元の本文", "退避する入力")
        val authError = assertThrows(HarmionException::class.java) {
            runBlocking { fixture.session.append(original.copy(identity = fixture.node.identity.copy(clientId = "other"))) }
        }
        assertEquals("long_authorization_changed", authError.code)
        assertEquals(0, fixture.node.reads)
        fixture.node.view = matching("変わった本文")
        val changed = assertThrows(HarmionException::class.java) { runBlocking { fixture.session.append(original) } }
        assertEquals("long_capture_changed", changed.code)
        assertTrue(fixture.node.sent.isEmpty())
    }

    private class Fixture {
        var record: LongCaptureRecord? = null
        val node = FakeNode { assertTrue(record?.pending != null) }
        val session = LongCaptureSession(node) { record = it }
        fun open(body: String, draft: String = "") = LongCaptureRecord("same-id", node.identity, body, draft)
    }

    private class FakeNode(private val onSave: () -> Unit) : LongCaptureNode {
        override val identity = AuthorizationIdentity("cluster", "node", "client")
        var view: LongCaptureView? = null
        var next: LongCaptureView? = null
        var loseReceipt = false
        var reads = 0
        val sent = mutableListOf<String>()
        override suspend fun readLong(id: String): LongCaptureView {
            reads++
            return view ?: throw HarmionException("http_404", "missing")
        }
        override fun createLongRequest(id: String, body: String) = "create:$id"
        override fun appendLongRequest(id: String, token: String, body: String) = "append:$id:$token"
        override fun finishLongRequest(id: String, token: String) = "finish:$id:$token"
        override suspend fun saveLong(request: String): String {
            onSave()
            sent += request
            view = next
            if (loseReceipt) throw HarmionException("transport", "lost response")
            return "receipt"
        }
    }

    private companion object {
        fun matching(body: String, open: Boolean = true) = LongCaptureView(body, open, false, "basis")
    }
}
