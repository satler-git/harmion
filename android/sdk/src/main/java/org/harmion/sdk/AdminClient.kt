package org.harmion.sdk

import org.harmion.sdk.core.HarmionNative
import org.harmion.sdk.core.nativeReply
import org.json.JSONArray
import org.json.JSONObject

/**
 * Owner-side client for the embedded node.
 *
 * Only the node app holds the administrative credential; this class exists so
 * the management UI and approval activity can reuse the same pinned client as
 * third-party apps instead of talking to SQLite directly.
 */
class HarmionAdminClient private constructor(private val handle: Long) : AutoCloseable {
    fun call(operation: String, body: ByteArray = ByteArray(0)): JSONObject =
        nativeReply(
            HarmionNative.callRaw(handle, operation, body.toHex()),
        ) {
            it.getJSONObject("response")
        }

    fun callJson(operation: String, body: JSONObject = JSONObject()): JSONObject =
        call(operation, body.toString().toByteArray(Charsets.UTF_8))

    fun connections(): List<JSONObject> {
        val response = call("connect-list")
        val array = response.optJSONArray("connections") ?: return emptyList()
        return (0 until array.length()).map { array.getJSONObject(it) }
    }

    fun decideConnection(connectionId: String, allow: Boolean, replace: Boolean = false): JSONObject =
        callJson(
            "connect-approve",
            JSONObject()
                .put("connection_id", connectionId)
                .put("allow", allow)
                .put("replace", replace),
        )

    fun ownerStatus(): JSONObject = call("owner-status")

    /** Asks the node for an owner challenge. The returned `statement` is hex-encoded. */
    fun ownerChallenge(
        operation: String,
        body: ByteArray = ByteArray(0),
        targetHex: String = "",
        sessionHex: String = "",
    ): JSONObject =
        nativeReply(HarmionNative.ownerChallenge(handle, operation, targetHex, body.toHex(), sessionHex)) {
            it.getJSONObject("response")
        }

    /** Submits an operation with an armored SSHSIG owner assertion. */
    fun callWithOwner(
        operation: String,
        body: ByteArray,
        challengeHex: String,
        signaturePem: String,
    ): JSONObject =
        nativeReply(HarmionNative.callWithOwner(handle, operation, body.toHex(), challengeHex, signaturePem)) {
            it.getJSONObject("response")
        }

    /** App credentials and their grants (management read). */
    fun apps(): List<JSONObject> =
        call("apps").optJSONArray("apps")?.toJsonObjectList() ?: emptyList()
    fun memberView(): JSONObject = call("members")

    /** Conflict view for one content. */
    fun conflicts(contentHex: String): JSONObject =
        nativeReply(HarmionNative.conflicts(handle, contentHex)) { it.getJSONObject("response") }

    /** Management conflict view (Manage token) required for three-way merge. */
    fun inspect(contentHex: String): JSONObject =
        nativeReply(HarmionNative.inspect(handle, contentHex)) { it.getJSONObject("response") }

    fun mergeInput(
        contentHex: String,
        tokenHex: String,
        leftHex: String,
        rightHex: String,
        baseHex: String = "",
    ): JSONObject =
        nativeReply(
            HarmionNative.mergeInput(handle, contentHex, tokenHex, leftHex, rightHex, baseHex),
        ) { it.getJSONObject("response") }

    fun mergePlan(
        contentHex: String,
        tokenHex: String,
        leftHex: String,
        rightHex: String,
        baseHex: String = "",
    ): JSONObject =
        nativeReply(
            HarmionNative.mergePlan(handle, contentHex, tokenHex, leftHex, rightHex, baseHex),
        ) { it.getJSONObject("response") }

    fun query(conditionJson: String, limit: Int = 32): JSONObject =
        nativeReply(HarmionNative.query(handle, conditionJson, limit)) { it }

    fun select(contentHex: String, tokenHex: String, selections: JSONArray): JSONObject =
        saveSpec(contentHex, JSONObject().put("type", "select").put("token", tokenHex).put("selections", selections))

    fun resolve(contentHex: String, tokenHex: String, changes: JSONArray): JSONObject =
        saveSpec(contentHex, JSONObject().put("type", "resolve").put("token", tokenHex).put("changes", changes))

    private fun saveSpec(contentHex: String, command: JSONObject): JSONObject {
        val request =
            JSONObject()
                .put("request_id", Requests.randomIdHex())
                .put("content_id", contentHex)
                .put("command", command)
        return nativeReply(HarmionNative.save(handle, request.toString())) {
            it.getJSONObject("response")
        }
    }

    fun metrics(): JSONObject = call("metrics")

    /** Replaces owner credentials using a one-time recovery code. */
    fun recoverOwner(recoveryCodeHex: String, publicKey: String, label: String = ""): JSONObject =
        call(
            "owner-recover",
            JSONObject()
                .put("recovery_code", recoveryCodeHex)
                .put("public_key", publicKey)
                .put("label", label)
                .toString()
                .toByteArray(Charsets.UTF_8),
        )

    fun identity(): JSONObject = call("identity")

    /** Joins a cluster using a peer registration (QR payload decoded by the caller). */
    fun join(registrationJson: ByteArray): JSONObject = call("join", registrationJson)

    fun provisionManifest(): JSONObject = call("provision-manifest")

    override fun close() {
        runCatching { HarmionNative.close(handle) }
    }

    companion object {
        fun connect(connection: NodeConnection, credentialHex: String): HarmionAdminClient {
            val handle =
                nativeReply(HarmionNative.connect(connection.discoveryJson(), credentialHex)) {
                    it.getLong("handle")
                }
            return HarmionAdminClient(handle)
        }
    }
}

internal fun JSONArray.toJsonObjectList(): List<JSONObject> =
    (0 until length()).map { getJSONObject(it) }
