package org.harmion.mote

/**
 * Bounded, foreground-only reconnect policy.
 *
 * It only decides whether a *connection* may be retried. It never resends a write:
 * the frozen capture request and its manual "再送" path stay untouched.
 */
internal object AutoReconnectPolicy {
    const val MaxAttempts = 2
    const val AttemptTimeoutMillis = 12_000L
    const val RetryDelayMillis = 2_000L
    const val SettleMillis = 300L

    fun shouldAttempt(connection: MoteConnection): Boolean =
        connection == MoteConnection.Unavailable || connection == MoteConnection.Failed

    fun isTerminal(connection: MoteConnection): Boolean = when (connection) {
        MoteConnection.Ready,
        MoteConnection.NeedsApproval,
        MoteConnection.Unavailable,
        MoteConnection.Failed -> true
        MoteConnection.Loading, MoteConnection.Approving -> false
    }
}
