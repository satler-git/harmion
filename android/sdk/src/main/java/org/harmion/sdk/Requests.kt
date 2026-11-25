package org.harmion.sdk

import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import org.json.JSONArray
import org.json.JSONObject

/** Grant filters requested by an app. */
sealed interface Filter {
    data object All : Filter

    data class Kinds(val kinds: List<String>) : Filter

    fun toJson(): JSONObject =
        when (this) {
            All -> JSONObject().put("type", "all")
            is Kinds -> JSONObject().put("type", "kinds").put("kinds", JSONArray(kinds))
        }
}

/** What the app asks the owner to approve. */
data class AuthorizationRequest(
    val read: Filter? = Filter.All,
    val write: Filter? = Filter.All,
    val call: Boolean = false,
    val providerNames: List<String> = emptyList(),
    val providers: List<JSONObject> = emptyList(),
    val providerReplace: Boolean = false,
) {
    init {
        require(read != null || write != null || call || providerNames.isNotEmpty()) {
            "request at least one grant"
        }
        require(!providerNames.isEmpty() || providers.isEmpty()) {
            "provider manifests require providerNames"
        }
    }

    internal fun toOfferJson(appName: String, challenge: String, state: String): JSONObject {
        val grants =
            JSONObject().apply {
                read?.let { put("read", it.toJson()) }
                write?.let { put("write", it.toJson()) }
                put("call", call)
                if (providerNames.isNotEmpty()) {
                    put("provide", true)
                    put("provide_names", JSONArray(providerNames))
                }
            }
        return JSONObject()
            .put("version", 1)
            .put("name", appName)
            .put("redirect_uri", REDIRECT_URI)
            .put("challenge", challenge)
            .put("state", state)
            .put("grants", grants)
            .put("providers", JSONArray(providers))
            .put("provider_replace", providerReplace)
    }

    companion object {
        /**
         * Loopback HTTP redirect required by the core's offer validation.
         * Native apps never receive the authorization code here: the code is
         * exchanged by the approval flow through the local SDK.
         */
        const val REDIRECT_URI: String = "http://127.0.0.1/harmion-sdk/callback"
    }
}

/** Helpers that build the JSON specs understood by the native bridge. */
object Requests {
    private val random = SecureRandom()

    fun randomIdHex(bytes: Int = 32): String =
        ByteArray(bytes).also(random::nextBytes).toHex()

    /** PKCE S256 verifier: 43 unreserved characters as required by the core. */
    fun pkceVerifier(): String {
        val bytes = ByteArray(32).also(random::nextBytes)
        return Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)
    }

    fun pkceChallenge(verifier: String): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray(Charsets.US_ASCII))
        return Base64.getUrlEncoder().withoutPadding().encodeToString(digest)
    }

    fun body(text: String): JSONObject = JSONObject().put("field", "body").put("text", text)

    fun attribute(namespace: String, key: String, value: Any?): JSONObject =
        JSONObject()
            .put("field", "attribute")
            .put("namespace", namespace)
            .put("key", key)
            .put("value", value ?: JSONObject.NULL)

    fun unsetAttribute(namespace: String, key: String): JSONObject =
        JSONObject()
            .put("field", "attribute")
            .put("namespace", namespace)
            .put("key", key)
            .put("absent", true)

    fun kind(name: String, member: Boolean = true): JSONObject =
        JSONObject().put("field", "kind").put("name", name).put("member", member)

    fun create(
        contentId: String,
        fields: List<JSONObject>,
        requestId: String = randomIdHex(),
    ): String = saveJson(requestId, contentId, "create", null, fields)

    fun edit(
        contentId: String,
        token: String,
        fields: List<JSONObject>,
        requestId: String = randomIdHex(),
    ): String = saveJson(requestId, contentId, "edit", token, fields)

    fun delete(
        contentId: String,
        token: String,
        requestId: String = randomIdHex(),
    ): String = saveJson(requestId, contentId, "delete", token, emptyList())

    fun restore(
        contentId: String,
        token: String,
        fields: List<JSONObject>,
        requestId: String = randomIdHex(),
    ): String = saveJson(requestId, contentId, "restore", token, fields)

    private fun saveJson(
        requestId: String,
        contentId: String,
        type: String,
        token: String?,
        fields: List<JSONObject>,
    ): String =
        JSONObject()
            .put("request_id", requestId)
            .put("content_id", contentId)
            .put(
                "command",
                JSONObject().apply {
                    put("type", type)
                    token?.let { put("token", it) }
                    if (type != "delete") put("fields", JSONArray(fields))
                },
            )
            .toString()

    fun and(vararg conditions: JSONObject): JSONObject =
        JSONObject().put("type", "and").put("conditions", JSONArray(conditions.toList()))

    fun or(vararg conditions: JSONObject): JSONObject =
        JSONObject().put("type", "or").put("conditions", JSONArray(conditions.toList()))

    fun not(condition: JSONObject): JSONObject = JSONObject().put("type", "not").put("condition", condition)

    fun kindIs(kind: String): JSONObject = JSONObject().put("type", "kind").put("kind", kind)

    fun attributeExists(namespace: String, key: String): JSONObject =
        attribute(namespace, key, JSONObject().put("type", "exists"))

    fun attributeEquals(namespace: String, key: String, value: Any?): JSONObject =
        attribute(
            namespace,
            key,
            JSONObject().put("type", "eq").put("value", value ?: JSONObject.NULL),
        )

    fun attributeCompare(
        namespace: String,
        key: String,
        operator: String,
        value: Any,
    ): JSONObject =
        attribute(
            namespace,
            key,
            JSONObject().put("type", "compare").put("operator", operator).put("value", value),
        )

    fun lastEditedAfter(utcMicros: Long): JSONObject =
        JSONObject()
            .put("type", "last_edited_at")
            .put("operator", "greater")
            .put("utc_micros", utcMicros)

    private fun attribute(namespace: String, key: String, predicate: JSONObject): JSONObject =
        JSONObject()
            .put("type", "attribute")
            .put("namespace", namespace)
            .put("key", key)
            .put("predicate", predicate)
}
