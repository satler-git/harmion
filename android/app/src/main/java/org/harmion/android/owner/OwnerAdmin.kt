package org.harmion.android.owner

import androidx.fragment.app.FragmentActivity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.harmion.android.core.NodeRuntimeController
import org.harmion.sdk.HarmionAdminClient
import org.json.JSONObject

/**
 * Runs node App API operations with an owner assertion when the node enforces
 * owner approval.
 *
 * The admin credential alone is not enough for owner-required operations; the
 * node issues a single-use challenge, the app shows a biometric prompt, signs
 * the canonical statement with the Keystore-wrapped owner seed, and submits the
 * SSHSIG. When enforcement is off (no owner enrolled) the plain call is used, so
 * first-run onboarding keeps working.
 */
class OwnerAdmin(
    private val controller: NodeRuntimeController,
    private val keys: OwnerKeyStore,
) {
    suspend fun call(
        admin: HarmionAdminClient,
        activity: FragmentActivity?,
        operation: String,
        body: ByteArray,
    ): JSONObject =
        withContext(Dispatchers.IO) {
            val enforced = runCatching { admin.ownerStatus().optBoolean("enforce") }.getOrDefault(false)
            if (!enforced) return@withContext admin.call(operation, body)
            val screen =
                activity
                    ?: throw IllegalStateException("所有者承認にはアプリの画面が必要です")
            if (!keys.hasKey) {
                throw IllegalStateException("所有者キーが登録されていません。設定から登録してください。")
            }
            val challenge = admin.ownerChallenge(operation, body)
            val seed = keys.unlock(screen) ?: throw IllegalStateException("所有者キーを復号できません")
            try {
                val signed = controller.ownerSign(seed, challenge.getString("statement"))
                admin.callWithOwner(
                    operation,
                    body,
                    challenge.getString("challenge"),
                    signed.getString("signature"),
                )
            } finally {
                // The seed is a Swift String; there is no zeroize on this side.
                // The native signer drops its copy immediately after signing.
                @Suppress("UNUSED_EXPRESSION") seed
            }
        }

    suspend fun decideConnection(
        admin: HarmionAdminClient,
        activity: FragmentActivity?,
        connectionId: String,
        allow: Boolean,
        expiresAt: String? = null,
    ): JSONObject =
        call(
            admin,
            activity,
            "connect-approve",
            JSONObject()
                .put("connection_id", connectionId)
                .put("allow", allow)
                .put("replace", false)
                .apply { expiresAt?.let { put("expires_at", it) } }
                .toString()
                .toByteArray(Charsets.UTF_8),
        )
}
