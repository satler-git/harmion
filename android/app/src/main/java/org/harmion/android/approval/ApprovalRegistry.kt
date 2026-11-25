package org.harmion.android.approval

import java.security.SecureRandom
import org.harmion.sdk.Hex

/**
 * Single-use approval tokens.
 *
 * The bound service issues a token only after verifying the Binder caller's UID,
 * package and signing certificate. The token is bound to that identity and one
 * connection request, lives briefly, and is consumed by the approval activity.
 * No token, connection secret or credential is ever placed in an Intent result.
 */
object ApprovalRegistry {
    data class Grant(
        val token: String,
        val connectionId: String,
        val uid: Int,
        val packageName: String,
        val certificateSha256: String,
        val expiresAtMillis: Long,
    )

    private const val TTL_MILLIS = 5 * 60 * 1000L
    private const val MAX_PENDING = 16
    private const val MAX_PENDING_PER_APP = 4

    private val random = SecureRandom()
    private val grants = mutableMapOf<String, Grant>()

    @Synchronized
    fun register(
        connectionId: String,
        uid: Int,
        packageName: String,
        certificateSha256: String,
    ): Grant? {
        prune()
        // Never evict a live grant: a malicious app could otherwise flush a
        // legitimate pending approval by filling the table.
        if (grants.size >= MAX_PENDING) return null
        if (grants.values.count { it.uid == uid } >= MAX_PENDING_PER_APP) return null
        val token = Hex.encode(ByteArray(32).also(random::nextBytes))
        val grant =
            Grant(
                token = token,
                connectionId = connectionId,
                uid = uid,
                packageName = packageName,
                certificateSha256 = certificateSha256,
                expiresAtMillis = System.currentTimeMillis() + TTL_MILLIS,
            )
        grants[token] = grant
        return grant
    }

    @Synchronized
    fun peek(token: String): Grant? {
        prune()
        return grants[token]
    }

    @Synchronized
    fun consume(token: String): Grant? {
        prune()
        return grants.remove(token)
    }

    private fun prune() {
        val now = System.currentTimeMillis()
        grants.entries.removeAll { it.value.expiresAtMillis <= now }
    }
}
