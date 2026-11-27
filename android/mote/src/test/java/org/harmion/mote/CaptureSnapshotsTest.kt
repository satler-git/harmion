package org.harmion.mote

import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.runBlocking
import org.harmion.sdk.HarmionException
import org.harmion.sdk.SubscriptionEvent
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptureSnapshotsTest {
    @Test
    fun publishesOnlyCompleteSnapshotsAndHidesTheCacheOnReset() {
        val result = mutableListOf<CaptureSnapshot>()
        runBlocking {
            captureSnapshots(flowOf(
                SubscriptionEvent.SnapshotChunk,
                SubscriptionEvent.SnapshotEnd(emptyMap()),
                SubscriptionEvent.ResetRequired,
                SubscriptionEvent.SnapshotChunk,
                SubscriptionEvent.SnapshotEnd(emptyMap()),
            )).collect(result::add)
        }
        assertEquals(listOf(
            CaptureSnapshot.Ready(emptyList()),
            CaptureSnapshot.Loading,
            CaptureSnapshot.Ready(emptyList()),
        ), result)
    }

    @Test
    fun doesNotPublishAnIncompleteDelta() {
        val result = mutableListOf<CaptureSnapshot>()
        val error = assertThrows(HarmionException::class.java) {
            runBlocking {
                captureSnapshots(flowOf(
                    SubscriptionEvent.SnapshotEnd(emptyMap()),
                    SubscriptionEvent.Changes(listOf("missing-id"), emptyList(), emptyMap()),
                )).collect(result::add)
            }
        }
        assertEquals("invalid_response", error.code)
        assertEquals(listOf(CaptureSnapshot.Ready(emptyList())), result)
    }

    @Test
    fun readsCanonicalScientificTimestampsWithoutCoercingStringsOrFractions() {
        assertEquals(1_795_644_249_000_000L, captureTimestamp(1.795644249e15))
        assertEquals(1_795_644_249_000_000L, captureTimestamp(1_795_644_249_000_000L))
        assertEquals(null, captureTimestamp("1795644249000000"))
        assertEquals(null, captureTimestamp(123.5))
    }

    @Test
    fun stopsAfterRepeatedResetsEvenIfSnapshotsInterleave() {
        val result = mutableListOf<CaptureSnapshot>()
        val events = List(4) { listOf(SubscriptionEvent.SnapshotEnd(emptyMap()), SubscriptionEvent.ResetRequired) }.flatten()
        val error = assertThrows(HarmionException::class.java) {
            runBlocking { captureSnapshots(flowOf(*events.toTypedArray())).collect(result::add) }
        }
        assertEquals("capture_reset_limit", error.code)
        assertEquals(CaptureSnapshot.Loading, result.last())
    }

    @Test
    fun readsReplyTargetOpenSessionAndArchiveTimeFromAttributes() {
        val replyTo = "b".repeat(64)
        val view = JSONObject()
            .put("edit_token", "token")
            .put("conflicts", JSONArray())
            .put("deletion_conflict", false)
            .put(
                "content",
                JSONObject()
                    .put("id", "id")
                    .put("body", "返信")
                    .put("kinds", JSONArray(listOf("capture")))
                    .put(
                        "attributes",
                        JSONArray(
                            listOf(
                                JSONObject().put("namespace", "capture").put("key", "reply_to").put("value", replyTo),
                                JSONObject().put("namespace", "capture").put("key", "session_open").put("value", true),
                                JSONObject().put("namespace", "capture").put("key", "archived").put("value", true),
                                JSONObject().put("namespace", "capture").put("key", "archived_at_micros")
                                    .put("value", 1_795_644_250_000_000L),
                            ),
                        ),
                    ),
            )
        val result = mutableListOf<CaptureSnapshot>()
        runBlocking {
            captureSnapshots(flowOf(SubscriptionEvent.SnapshotEnd(mapOf("id" to view)))).collect(result::add)
        }
        val capture = (result.single() as CaptureSnapshot.Ready).items.single()
        assertEquals(replyTo, capture.replyTo)
        assertTrue(capture.sessionOpen)
        assertTrue(capture.isLong)
        assertTrue(capture.archived)
        assertEquals(1_795_644_250_000_000L, capture.archivedAtMicros)
    }
}
