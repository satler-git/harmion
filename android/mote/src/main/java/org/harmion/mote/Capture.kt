package org.harmion.mote

/** Read projection of one `capture` Content, not a payload to write back in full. */
data class Capture(
    val id: String,
    val body: String,
    /** UTC Unix microseconds recorded on create. Other writers may omit this attribute. */
    val createdAtMicros: Long?,
    val archived: Boolean,
    val pinned: Boolean,
    /** Opaque edit token of the displayed Harmion read view; it is not a credential. */
    val token: String,
    val sessionOpen: Boolean = false,
    /** Finished long captures retain the session attribute with a false value. */
    val isLong: Boolean = sessionOpen,
    /** Capture ID this one replies to, if any. The target may have been deleted. */
    val replyTo: String? = null,
    /**
     * UTC Unix microseconds recorded when the capture was archived.
     * Absent for captures archived before this attribute was introduced.
     */
    val archivedAtMicros: Long? = null,
) {
    // Avoid including private capture text or the node-local edit token in diagnostic logs.
    override fun toString(): String = "Capture(id=$id)"
}

private val inboxOrder = compareBy<Capture>(Capture::createdAtMicros, Capture::id)

/**
 * Oldest archive first. Captures without an archive time sort first, by creation
 * time, so they stay at the top instead of being mixed into the dated sequence.
 */
private val archiveOrder = compareBy<Capture> { it.archivedAtMicros ?: Long.MIN_VALUE }
    .thenBy(Capture::createdAtMicros)
    .thenBy(Capture::id)

/**
 * Inbox shows unarchived captures. Archived shows only archived captures in
 * archive order. Pinning filters either list; it never changes the order.
 */
data class CaptureFilter(val archivedOnly: Boolean = false, val pinnedOnly: Boolean = false) {
    fun applyTo(captures: Iterable<Capture>): List<Capture> =
        captures.filter { it.archived == archivedOnly && (!pinnedOnly || it.pinned) }
            .sortedWith(if (archivedOnly) archiveOrder else inboxOrder)
}
