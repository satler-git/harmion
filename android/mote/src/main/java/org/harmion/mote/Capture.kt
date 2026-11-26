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
) {
    // Avoid including private capture text or the node-local edit token in diagnostic logs.
    override fun toString(): String = "Capture(id=$id)"
}

/** Independent Inbox/All and Pinned controls; neither pinning nor archiving affects order. */
data class CaptureFilter(val includeArchived: Boolean = false, val pinnedOnly: Boolean = false) {
    /** Oldest first. Undated records sort first; Content ID breaks ties across snapshot orders. */
    fun applyTo(captures: Iterable<Capture>): List<Capture> =
        captures.filter { (includeArchived || !it.archived) && (!pinnedOnly || it.pinned) }
            .sortedWith(compareBy(Capture::createdAtMicros, Capture::id))
}
