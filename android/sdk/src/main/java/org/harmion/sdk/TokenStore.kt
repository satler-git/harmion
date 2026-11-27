package org.harmion.sdk

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.json.JSONObject

/** Tokens plus the node they belong to. */
internal data class StoredCredentials(
    val nodeId: String,
    val clusterId: String,
    val clientId: String,
    val accessToken: String,
    val refreshToken: String,
    val expiresAtMillis: Long,
    val scope: String,
)

/** In-flight PKCE approval that can be resumed after process death. */
internal data class PendingApproval(
    val connectionId: String,
    val requestSecret: String,
    val verifier: String,
    val nodeId: String,
    val clusterId: String,
    val createdAtMillis: Long,
)

/**
 * App-private token storage.
 *
 * The access and refresh tokens are encrypted with a non-exportable Android
 * Keystore key. No backup copies them: a new device must re-connect through the
 * approval flow instead of silently reusing an old authorization.
 */
internal class TokenStore(context: Context) {
    private val preferences =
        context.getSharedPreferences("org.harmion.sdk.tokens", Context.MODE_PRIVATE)

    fun load(): StoredCredentials? {
        val encrypted = preferences.getString(KEY_TOKENS, null) ?: return null
        return try {
            val json = JSONObject(decrypt(encrypted))
            StoredCredentials(
                nodeId = json.getString("node_id"),
                clusterId = json.getString("cluster_id"),
                clientId = json.getString("client_id"),
                accessToken = json.getString("access_token"),
                refreshToken = json.getString("refresh_token"),
                expiresAtMillis = json.getLong("expires_at"),
                scope = json.optString("scope"),
            )
        } catch (error: Exception) {
            // A rotated or invalidated Keystore key makes stored tokens unusable.
            clear()
            null
        }
    }

    fun save(credentials: StoredCredentials) {
        val json =
            JSONObject()
                .put("node_id", credentials.nodeId)
                .put("cluster_id", credentials.clusterId)
                .put("client_id", credentials.clientId)
                .put("access_token", credentials.accessToken)
                .put("refresh_token", credentials.refreshToken)
                .put("expires_at", credentials.expiresAtMillis)
                .put("scope", credentials.scope)
                .toString()
        preferences.edit().putString(KEY_TOKENS, encrypt(json)).apply()
    }

    fun clear() {
        preferences.edit().remove(KEY_TOKENS).apply()
    }

    fun loadPendingApproval(): PendingApproval? {
        val encrypted = preferences.getString(KEY_PENDING, null) ?: return null
        return try {
            val json = JSONObject(decrypt(encrypted))
            PendingApproval(
                connectionId = json.getString("connection_id"),
                requestSecret = json.getString("request_secret"),
                verifier = json.getString("verifier"),
                nodeId = json.getString("node_id"),
                clusterId = json.getString("cluster_id"),
                createdAtMillis = json.getLong("created_at"),
            )
        } catch (error: Exception) {
            clearPendingApproval()
            null
        }
    }

    fun savePendingApproval(pending: PendingApproval) {
        val json =
            JSONObject()
                .put("connection_id", pending.connectionId)
                .put("request_secret", pending.requestSecret)
                .put("verifier", pending.verifier)
                .put("node_id", pending.nodeId)
                .put("cluster_id", pending.clusterId)
                .put("created_at", pending.createdAtMillis)
                .toString()
        preferences.edit().putString(KEY_PENDING, encrypt(json)).apply()
    }

    fun clearPendingApproval() {
        preferences.edit().remove(KEY_PENDING).apply()
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (store.getEntry(KEY_ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    private fun encrypt(plain: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val sealed = cipher.iv + cipher.doFinal(plain.toByteArray(Charsets.UTF_8))
        return Base64.encodeToString(sealed, Base64.NO_WRAP)
    }

    private fun decrypt(encoded: String): String {
        val sealed = Base64.decode(encoded, Base64.NO_WRAP)
        val iv = sealed.copyOfRange(0, IV_BYTES)
        val body = sealed.copyOfRange(IV_BYTES, sealed.size)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, iv))
        return String(cipher.doFinal(body), Charsets.UTF_8)
    }

    private companion object {
        const val KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "org.harmion.sdk.tokens"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
        const val KEY_TOKENS = "credentials"
        const val KEY_PENDING = "pending_approval"
    }
}
