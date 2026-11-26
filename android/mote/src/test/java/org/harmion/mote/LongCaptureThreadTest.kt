package org.harmion.mote

import org.junit.Assert.assertEquals
import org.junit.Test

class LongCaptureThreadTest {
    @Test
    fun keepsA1024CharacterParagraphTogether() {
        val sample = "えっと、もう少し考えてみる。".repeat(80).take(1024)
        val following = "次の入力を、ここへ書き足す。"
        val body = "$sample\n\n$following"

        val slices = threadSlices(body)

        assertEquals(listOf(sample, following), slices.map { body.substring(it.start, it.end) })
        assertEquals(listOf(true, true), slices.map(ThreadSlice::startsBlock))
    }
}
