package org.harmion.mote

import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureFilterTest {
    private val active = Capture("a", "one", 20L, archived = false, pinned = false, token = "1")
    private val activePin = active.copy(id = "b", createdAtMicros = 30L, pinned = true)
    private val archived = active.copy(id = "c", createdAtMicros = 40L, archived = true)
    private val archivedPin = active.copy(id = "d", createdAtMicros = 10L, archived = true, pinned = true)
    private val captures = listOf(activePin, archived, active, archivedPin)

    @Test
    fun inboxAndPinnedAreIndependent() {
        assertEquals(listOf(active, activePin), CaptureFilter().applyTo(captures))
        assertEquals(listOf(activePin), CaptureFilter(pinnedOnly = true).applyTo(captures))
        assertEquals(
            listOf(archivedPin, active, activePin, archived),
            CaptureFilter(includeArchived = true).applyTo(captures),
        )
        assertEquals(
            listOf(archivedPin, activePin),
            CaptureFilter(includeArchived = true, pinnedOnly = true).applyTo(captures),
        )
    }

    @Test
    fun missingTimesAndTiesHaveDeterministicOrderAcrossSnapshots() {
        val undated = active.copy(id = "z", createdAtMicros = null)
        val sameTime = active.copy(id = "b", pinned = true)
        val unordered = listOf(sameTime, active, undated)
        val expected = listOf(undated, active, sameTime)
        assertEquals(expected, CaptureFilter().applyTo(unordered))
        assertEquals(expected, CaptureFilter().applyTo(unordered.reversed()))
    }
}
