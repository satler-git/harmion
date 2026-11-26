package org.harmion.mote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SharedLinkDraftTest {
    @Test
    fun preservesTheOriginalUrlAndMultilineNote() {
        val url = "https://example.org/a%20b?next=%2Fa#heading"
        val link = checkNotNull(sharedLinkDraft(url))
        val note = "あとで読む\n\n入力欄の動きを確認する"
        val draft = link.withNote(note)

        assertEquals("example.org", link.host)
        assertEquals(SharedLinkDraft(url, "example.org", note), sharedLinkDraft(draft))
        assertEquals(url, sharedLinkDraft(draft)?.withNote(""))
    }

    @Test
    fun mixedTextSharesKeepTheirNormalEditor() {
        assertNull(sharedLinkDraft("Compose の資料\nhttps://example.org/compose"))
        assertNull(sharedLinkDraft("content://example.org/compose"))
    }
}
