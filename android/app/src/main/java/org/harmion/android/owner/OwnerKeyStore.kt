package org.harmion.android.owner

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext

/** The owner cancelled a biometric prompt. Not an error worth reporting. */
class OwnerCancelled : IllegalStateException("biometric authentication cancelled")

/**
 * Owner key material kept inside the Android Keystore boundary.
 *
 * The Ed25519 seed is generated in this process, encrypted by an AES-GCM key that
 * requires biometric authentication for every use, and only the ciphertext is
 * stored. Signing happens in the native node (SSHSIG), so the seed is decrypted
 * for one signing operation and then dropped; it is never written elsewhere.
 *
 * The seed is the recovery-material equivalent: losing it (for example, clearing
 * app data) requires the printed recovery code and a re-enrollment.
 */
class OwnerKeyStore(context: Context) {
    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    private val appContext = context.applicationContext
    private val executor = ContextCompat.getMainExecutor(appContext)

    val hasKey: Boolean
        get() = prefs.contains(KEY_SEED) && prefs.contains(KEY_IV)

    val publicKey: String?
        get() = prefs.getString(KEY_PUBLIC, null)

    val fingerprint: String?
        get() = prefs.getString(KEY_FINGERPRINT, null)

    val enrolled: Boolean
        get() = prefs.getBoolean(KEY_ENROLLED, false)

    fun storePublicKey(publicKey: String, fingerprint: String) {
        prefs.edit()
            .putString(KEY_PUBLIC, publicKey)
            .putString(KEY_FINGERPRINT, fingerprint)
            .apply()
    }

    fun markEnrolled() {
        prefs.edit().putBoolean(KEY_ENROLLED, true).apply()
    }

    fun clear() {
        prefs.edit().clear().apply()
        runCatching {
            val store = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
            if (store.containsAlias(ALIAS)) store.deleteEntry(ALIAS)
        }
    }

    /**
     * Encrypts [seedHex] with a fresh Keystore key after biometric authentication.
     * Call [storePublicKey] with the matching public key first; on failure the
     * caller should [clear].
     */
    suspend fun create(activity: FragmentActivity, seedHex: String) =
        withContext(Dispatchers.Main) {
            val cipher =
                Cipher.getInstance(TRANSFORMATION).apply {
                    init(Cipher.ENCRYPT_MODE, ensureKey(activity))
                }
            val result = authenticate(activity, "所有者キーを保存", cipher)
            val used = result.cryptoObject?.cipher ?: cipher
            val encrypted = used.doFinal(org.harmion.sdk.Hex.decode(seedHex))
            prefs.edit()
                .putString(KEY_SEED, Base64.encodeToString(encrypted, Base64.NO_WRAP))
                .putString(KEY_IV, Base64.encodeToString(used.iv, Base64.NO_WRAP))
                .apply()
        }

    /** Decrypts the seed after biometric authentication. Null when no key exists. */
    suspend fun unlock(activity: FragmentActivity): String? =
        withContext(Dispatchers.Main) {
            val encrypted = prefs.getString(KEY_SEED, null) ?: return@withContext null
            val iv = prefs.getString(KEY_IV, null) ?: return@withContext null
            val cipher =
                Cipher.getInstance(TRANSFORMATION).apply {
                    init(
                        Cipher.DECRYPT_MODE,
                        keystore().getKey(ALIAS, null),
                        GCMParameterSpec(128, Base64.decode(iv, Base64.NO_WRAP)),
                    )
                }
            val result = authenticate(activity, "所有者承認", cipher)
            val used = result.cryptoObject?.cipher ?: cipher
            org.harmion.sdk.Hex.encode(used.doFinal(Base64.decode(encrypted, Base64.NO_WRAP)))
        }

    private suspend fun authenticate(
        activity: FragmentActivity,
        title: String,
        cipher: Cipher,
    ): BiometricPrompt.AuthenticationResult =
        suspendCancellableCoroutine { continuation ->
            val prompt =
                BiometricPrompt(
                    activity,
                    executor,
                    object : BiometricPrompt.AuthenticationCallback() {
                        override fun onAuthenticationSucceeded(
                            result: BiometricPrompt.AuthenticationResult,
                        ) {
                            if (continuation.isActive) continuation.resume(result)
                        }

                        override fun onAuthenticationError(code: Int, message: CharSequence) {
                            if (!continuation.isActive) return
                            val cancelled =
                                code == BiometricPrompt.ERROR_NEGATIVE_BUTTON ||
                                    code == BiometricPrompt.ERROR_USER_CANCELED ||
                                    code == BiometricPrompt.ERROR_CANCELED
                            continuation.resumeWithException(
                                if (cancelled) OwnerCancelled() else IllegalStateException(message.toString()),
                            )
                        }

                        // A failed attempt keeps the prompt open; the platform
                        // controls retry limits and lockout.
                        override fun onAuthenticationFailed() {}
                    },
                )
            continuation.invokeOnCancellation { prompt.cancelAuthentication() }
            prompt.authenticate(
                BiometricPrompt.PromptInfo.Builder()
                    .setTitle(title)
                    .setSubtitle("Harmionの所有者操作を承認します")
                    .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                    .setNegativeButtonText("キャンセル")
                    .build(),
                BiometricPrompt.CryptoObject(cipher),
            )
        }

    private fun ensureKey(activity: FragmentActivity): SecretKey {
        val store = keystore()
        if (store.containsAlias(ALIAS)) {
            return store.getKey(ALIAS, null) as SecretKey
        }
        // If the platform has no strong biometric, fail before creating an
        // unusable auth-bound key.
        val available =
            BiometricManager.from(activity)
                .canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG)
        if (available != BiometricManager.BIOMETRIC_SUCCESS) {
            throw IllegalStateException("この端末では生体認証を利用できません（$available）")
        }
        val builder =
            KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setUserAuthenticationRequired(true)
        builder.setInvalidatedByBiometricEnrollment(true)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            builder.setUserAuthenticationParameters(
                0,
                KeyProperties.AUTH_BIOMETRIC_STRONG,
            )
        } else {
            @Suppress("DEPRECATION")
            builder.setUserAuthenticationValidityDurationSeconds(-1)
        }
        val generator =
            KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        generator.init(builder.build())
        return generator.generateKey()
    }

    private fun keystore(): KeyStore =
        KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }

    private companion object {
        const val ANDROID_KEYSTORE = "AndroidKeyStore"
        const val ALIAS = "harmion-owner"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val PREFS = "owner_key"
        const val KEY_SEED = "seed"
        const val KEY_IV = "iv"
        const val KEY_PUBLIC = "public_key"
        const val KEY_FINGERPRINT = "fingerprint"
        const val KEY_ENROLLED = "enrolled"
    }
}
