package org.harmion.mote

import android.os.SystemClock
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.hapticfeedback.HapticFeedback
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback

/**
 * The fixed vocabulary of touch feedback in Mote.
 *
 * Every entry is a platform semantic rather than a vibration recipe, so the device keeps
 * its own tuning and the system "touch feedback" setting still applies.
 */
internal enum class MoteHaptic(val type: HapticFeedbackType) {
    /** A swipe crossed the distance that commits it. */
    Threshold(HapticFeedbackType.GestureThresholdActivate),

    /** A write the user started has landed, or a destructive action completed. */
    Confirm(HapticFeedbackType.Confirm),

    /** A toggle switched on, such as pinning or the pinned filter. */
    ToggleOn(HapticFeedbackType.ToggleOn),

    /** A toggle switched off. */
    ToggleOff(HapticFeedbackType.ToggleOff),

    /** A light change, such as selecting a capture or changing the timeline filter. */
    Tick(HapticFeedbackType.SegmentTick),
}

internal class MoteHaptics(
    private val feedback: HapticFeedback,
    private val enabled: Boolean,
) {
    private var lastThresholdAt = 0L

    fun perform(haptic: MoteHaptic) {
        if (!enabled) return
        // A finger resting on the swipe threshold can flip targetValue back and forth.
        // One arm pulse per short interval is enough; the rest would be noise.
        if (haptic == MoteHaptic.Threshold) {
            val now = SystemClock.uptimeMillis()
            if (now - lastThresholdAt < ThresholdCooldownMillis) return
            lastThresholdAt = now
        }
        feedback.performHapticFeedback(haptic.type)
    }

    fun perform(event: MoteHapticEvent) {
        perform(event.toHaptic())
    }

    private companion object {
        const val ThresholdCooldownMillis = 250L
    }
}

@Composable
internal fun rememberMoteHaptics(enabled: Boolean): MoteHaptics {
    val feedback = LocalHapticFeedback.current
    return remember(feedback, enabled) { MoteHaptics(feedback, enabled) }
}

private fun MoteHapticEvent.toHaptic(): MoteHaptic = when (this) {
    MoteHapticEvent.Saved, MoteHapticEvent.Deleted, MoteHapticEvent.LongFinished -> MoteHaptic.Confirm
    MoteHapticEvent.LongAppended -> MoteHaptic.Tick
}
