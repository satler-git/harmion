package org.harmion.mote

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AutoReconnectPolicyTest {
    @Test
    fun retriesOnlyAfterATransientConnectionFailure() {
        assertTrue(AutoReconnectPolicy.shouldAttempt(MoteConnection.Unavailable))
        assertTrue(AutoReconnectPolicy.shouldAttempt(MoteConnection.Failed))
    }

    @Test
    fun neverRetriesWhileConnectedOrWaitingForTheOwner() {
        assertFalse(AutoReconnectPolicy.shouldAttempt(MoteConnection.Ready))
        assertFalse(AutoReconnectPolicy.shouldAttempt(MoteConnection.NeedsApproval))
        assertFalse(AutoReconnectPolicy.shouldAttempt(MoteConnection.Loading))
        assertFalse(AutoReconnectPolicy.shouldAttempt(MoteConnection.Approving))
    }

    @Test
    fun terminalStatesAreOnlyTheOnesTheWaitCanObserve() {
        assertTrue(AutoReconnectPolicy.isTerminal(MoteConnection.Ready))
        assertTrue(AutoReconnectPolicy.isTerminal(MoteConnection.NeedsApproval))
        assertTrue(AutoReconnectPolicy.isTerminal(MoteConnection.Unavailable))
        assertTrue(AutoReconnectPolicy.isTerminal(MoteConnection.Failed))
        assertFalse(AutoReconnectPolicy.isTerminal(MoteConnection.Loading))
        assertFalse(AutoReconnectPolicy.isTerminal(MoteConnection.Approving))
    }

    @Test
    fun attemptsStaySmallAndBounded() {
        assertTrue(AutoReconnectPolicy.MaxAttempts in 1..3)
        assertTrue(AutoReconnectPolicy.AttemptTimeoutMillis in 1_000..20_000)
        assertTrue(AutoReconnectPolicy.RetryDelayMillis in 0..10_000)
    }
}
