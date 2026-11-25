package org.harmion.sdk.core

import org.harmion.sdk.HarmionException
import org.json.JSONObject

/**
 * JNI bridge to `libharmion_sdk_native.so`.
 *
 * Every call is JSON in/out. The Kotlin side builds requests with
 * [org.harmion.sdk.Requests] and never hand-encodes HRQ/HQU bodies: the native
 * client shares the core's canonical encoders.
 */
internal object HarmionNative {
    init {
        System.loadLibrary("harmion_sdk_native")
    }

    external fun connect(connection: String, credential: String): String

    external fun anonymous(connection: String): String

    external fun close(handle: Long): String

    external fun callRaw(handle: Long, operation: String, bodyHex: String): String

    external fun save(handle: Long, request: String): String

    external fun get(handle: Long, content: String): String

    external fun query(handle: Long, condition: String, limit: Int): String

    external fun subscribe(handle: Long, condition: String): String

    external fun subscriptionNext(handle: Long, subscription: Long): String

    external fun unsubscribe(handle: Long, subscription: Long): String

    external fun connectStart(handle: Long, offer: String): String

    external fun connectStatus(handle: Long, request: String): String

    external fun connectComplete(handle: Long, request: String): String

    external fun connectList(handle: Long): String

    external fun connectApprove(handle: Long, approval: String): String

    external fun syncStatus(handle: Long, update: String): String

    external fun oauthRefresh(
        origin: String,
        root: String,
        clientId: String,
        refreshToken: String,
    ): String

    external fun randomHex(count: Int): String

    external fun conflicts(handle: Long, content: String): String

    external fun mergeInput(
        handle: Long,
        content: String,
        token: String,
        left: String,
        right: String,
        base: String,
    ): String

    external fun mergePlan(
        handle: Long,
        content: String,
        token: String,
        left: String,
        right: String,
        base: String,
    ): String

    external fun ownerChallenge(
        handle: Long,
        operation: String,
        target: String,
        body: String,
        session: String,
    ): String

    external fun callWithOwner(
        handle: Long,
        operation: String,
        body: String,
        challenge: String,
        signature: String,
    ): String
}

internal inline fun <T> nativeReply(raw: String, parse: (JSONObject) -> T): T {
    val json =
        try {
            JSONObject(raw)
        } catch (error: Exception) {
            throw HarmionException("bridge", "malformed native reply: ${error.message}", error)
        }
    if (!json.optBoolean("ok")) {
        val error = json.optJSONObject("error")
        throw HarmionException(
            error?.optString("code").orEmpty().ifEmpty { "unknown" },
            error?.optString("message").orEmpty(),
        )
    }
    return parse(json)
}

internal fun bridgeErrorCode(error: HarmionException): String = error.code
