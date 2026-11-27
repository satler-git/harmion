package org.harmion.mote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.io.IOException

class CaptureCacheTest {
    @Test
    fun roundTripsACachedSnapshotWithoutEditTokens() {
        val captures = listOf(
            Capture(
                id = "a".repeat(64),
                body = "一行目\n二行目",
                createdAtMicros = 1_795_644_249_000_000L,
                archived = true,
                pinned = true,
                token = "node-token",
                isLong = true,
                replyTo = "b".repeat(64),
            ),
            Capture(
                id = "c".repeat(64),
                body = "",
                createdAtMicros = null,
                archived = false,
                pinned = false,
                token = "node-token",
            ),
        )
        val decoded = CaptureCacheCodec.decode(CaptureCacheCodec.encode(captures))
        assertEquals(captures.map { it.copy(token = "") }, decoded.items)
    }

    @Test
    fun rejectsMalformedOrUnsupportedCache() {
        assertThrows(IOException::class.java) { CaptureCacheCodec.decode("not json".toByteArray()) }
        val encoded = CaptureCacheCodec.encode(emptyList())
        val unsupported = String(encoded, Charsets.UTF_8).replace("\"version\":1", "\"version\":2")
        assertThrows(IOException::class.java) { CaptureCacheCodec.decode(unsupported.toByteArray()) }
    }

    @Test
    fun refillsAnEmptySnapshot() {
        assertEquals(emptyList<Capture>(), CaptureCacheCodec.decode(CaptureCacheCodec.encode(emptyList())).items)
    }
}
