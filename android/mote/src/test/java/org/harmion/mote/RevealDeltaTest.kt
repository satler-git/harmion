package org.harmion.mote

import org.junit.Assert.assertEquals
import org.junit.Test

class RevealDeltaTest {
    private val safeTop = 100
    private val safeBottom = 1000
    private val margin = 10

    @Test
    fun alreadyVisibleItemDoesNotMove() {
        assertEquals(0, revealDelta(itemOffset = 300, itemSize = 120, safeTop, safeBottom, margin))
        assertEquals(0, revealDelta(itemOffset = 110, itemSize = 120, safeTop, safeBottom, margin))
        assertEquals(0, revealDelta(itemOffset = 870, itemSize = 120, safeTop, safeBottom, margin))
    }

    @Test
    fun itemUnderTheDockMovesOnlyFarEnoughToClearIt() {
        // Bottom is 1040; the safe bottom is 990.
        assertEquals(50, revealDelta(itemOffset = 940, itemSize = 100, safeTop, safeBottom, margin))
    }

    @Test
    fun itemUnderTheHeaderMovesDownOnlyFarEnough() {
        // Top is 80; the safe top is 110.
        assertEquals(-30, revealDelta(itemOffset = 80, itemSize = 20, safeTop, safeBottom, margin))
    }

    @Test
    fun itemTallerThanTheSafeBandKeepsItsBeginningVisible() {
        assertEquals(0, revealDelta(itemOffset = 110, itemSize = 2000, safeTop, safeBottom, margin))
        assertEquals(-10, revealDelta(itemOffset = 100, itemSize = 2000, safeTop, safeBottom, margin))
        assertEquals(40, revealDelta(itemOffset = 150, itemSize = 2000, safeTop, safeBottom, margin))
    }
}
