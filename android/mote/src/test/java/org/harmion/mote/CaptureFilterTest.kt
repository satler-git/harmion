package org.harmion.mote

import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureFilterTest {
    private val active = Capture("a", "one", 20L, archived = false, pinned = false, token = "1")
    private val activePin = active.copy(id = "b", createdAtMicros = 30L, pinned = true)
    private val archivedEarly = active.copy(
        id = "c",
        createdAtMicros = 40L,
        archived = true,
        archivedAtMicros = 100L,
    )
    private val archivedLate = active.copy(
        id = "d",
        createdAtMicros = 10L,
        archived = true,
        archivedAtMicros = 300L,
        pinned = true,
    )
    private val archivedLegacy = active.copy(
        id = "e",
        createdAtMicros = 50L,
        archived = true,
        archivedAtMicros = null,
    )
    private val captures = listOf(archivedLate, activePin, archivedLegacy, archivedEarly, active)

    @Test
    fun inboxHidesArchivedAndKeepsCreationOrder() {
        assertEquals(listOf(active, activePin), CaptureFilter().applyTo(captures))
        assertEquals(listOf(activePin), CaptureFilter(pinnedOnly = true).applyTo(captures))
    }

    @Test
    fun archivedUsesArchiveTimeOldestFirst() {
        assertEquals(
            listOf(archivedLegacy, archivedEarly, archivedLate),
            CaptureFilter(archivedOnly = true).applyTo(captures),
        )
        assertEquals(
            listOf(archivedLate),
            CaptureFilter(archivedOnly = true, pinnedOnly = true).applyTo(captures),
        )
    }

    @Test
    fun undatedArchivesSortFirstByCreationTime() {
        val olderLegacy = archivedLegacy.copy(id = "f", createdAtMicros = 5L)
        val unordered = listOf(archivedEarly, archivedLegacy, olderLegacy)
        assertEquals(
            listOf(olderLegacy, archivedLegacy, archivedEarly),
            CaptureFilter(archivedOnly = true).applyTo(unordered),
        )
    }

    @Test
    fun archiveTiesUseCreationTimeThenId() {
        val first = archivedLate.copy(id = "g", createdAtMicros = 10L, archivedAtMicros = 200L)
        val second = archivedLate.copy(id = "h", createdAtMicros = 20L, archivedAtMicros = 200L)
        assertEquals(
            listOf(first, second),
            CaptureFilter(archivedOnly = true).applyTo(listOf(second, first)),
        )
    }

    @Test
    fun archiveTiesUseIdWhenCreationTimeAlsoTies() {
        val first = archivedLate.copy(id = "g", createdAtMicros = 10L, archivedAtMicros = 200L)
        val second = archivedLate.copy(id = "h", createdAtMicros = 10L, archivedAtMicros = 200L)
        assertEquals(
            listOf(first, second),
            CaptureFilter(archivedOnly = true).applyTo(listOf(second, first)),
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
