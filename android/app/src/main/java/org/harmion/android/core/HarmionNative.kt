package org.harmion.android.core

import org.harmion.sdk.NodeConnection
import org.harmion.sdk.NodeStatus
import org.json.JSONObject

internal class NativeFailure(val code: String, message: String) : Exception(message)

internal class NativeReply(val json: JSONObject) {
    val ok: Boolean = json.optBoolean("ok")

    val code: String
        get() = json.optJSONObject("error")?.optString("code").orEmpty()

    val message: String
        get() = json.optJSONObject("error")?.optString("message").orEmpty()

    fun require(): JSONObject {
        if (!ok) throw NativeFailure(code.ifEmpty { "unknown" }, message)
        return json
    }
}

internal object HarmionNative {
    init {
        System.loadLibrary("harmion_android")
    }

    external fun initialize(root: String, options: String): String

    external fun start(root: String, options: String): String

    external fun stop(handle: Long): String

    external fun status(handle: Long): String

    external fun syncNow(handle: Long): String

    external fun syncGated(handle: Long): String

    external fun setSyncPolicy(handle: Long, permitted: Boolean, reason: String): String

    external fun requestSync(handle: Long, trigger: String): String

    external fun identifiers(root: String): String

    external fun connection(root: String): String

    external fun config(root: String): String

    external fun oauthRoot(root: String): String

    external fun ownerPublicKey(seedHex: String): String

    external fun ownerSign(seedHex: String, statementHex: String): String

    external fun ownerEnroll(handle: Long, publicKey: String, label: String): String
}

internal fun native(raw: String): NativeReply =
    try {
        NativeReply(JSONObject(raw))
    } catch (error: Exception) {
        throw NativeFailure("bridge", "malformed native reply: ${error.message}")
    }

/** Snapshot rendered by the management UI. */
data class RuntimeSnapshot(
    val running: Boolean,
    val clusterId: String,
    val nodeId: String,
    val appEndpoint: String,
    val membership: String,
    val peers: Int,
    val syncEnabled: Boolean,
    val syncPermitted: Boolean,
    val syncBlockedCode: String?,
    val syncBlockedMessage: String?,
    val inflight: List<String>,
    val retrying: Int,
    val lastSuccessAtMicros: Long?,
    val lastError: String?,
    val databaseBytes: Long,
) {
    companion object {
        fun fromJson(json: JSONObject): RuntimeSnapshot {
            val sync = json.optJSONObject("sync") ?: JSONObject()
            val blocked = sync.optJSONObject("blocked")
            val retrying = sync.optJSONObject("retrying")?.length() ?: 0
            val inflight =
                sync.optJSONArray("inflight")?.let { array ->
                    (0 until array.length()).map { array.getString(it) }
                } ?: emptyList()
            return RuntimeSnapshot(
                running = json.optBoolean("running"),
                clusterId = json.optString("cluster_id"),
                nodeId = json.optString("node_id"),
                appEndpoint = json.optString("app_endpoint"),
                membership = json.optString("membership", "unknown"),
                peers = json.optInt("peers", 0),
                syncEnabled = sync.optBoolean("enabled"),
                syncPermitted = sync.optBoolean("permitted", true),
                syncBlockedCode = blocked?.optString("code")?.ifEmpty { null },
                syncBlockedMessage = blocked?.optString("message")?.ifEmpty { null },
                inflight = inflight,
                retrying = retrying,
                lastSuccessAtMicros =
                    if (sync.has("last_success_at") && !sync.isNull("last_success_at")) {
                        sync.optLong("last_success_at")
                    } else {
                        null
                    },
                lastError =
                    if (sync.has("last_error") && !sync.isNull("last_error")) {
                        sync.optString("last_error").ifEmpty { null }
                    } else {
                        null
                    },
                databaseBytes = json.optJSONObject("storage")?.optLong("database_bytes") ?: 0L,
            )
        }

        fun fromStatus(status: NodeStatus): RuntimeSnapshot =
            RuntimeSnapshot(
                running = status.running,
                clusterId = status.clusterId,
                nodeId = status.nodeId,
                appEndpoint = "",
                membership = status.membership,
                peers = status.peers,
                syncEnabled = status.syncEnabled,
                syncPermitted = status.syncPermitted,
                syncBlockedCode = status.syncBlockedCode,
                syncBlockedMessage = status.syncBlockedMessage,
                inflight = emptyList(),
                retrying = 0,
                lastSuccessAtMicros = status.lastSuccessAtMicros,
                lastError = status.lastError,
                databaseBytes = 0L,
            )
    }
}

/** Discovery data plus the OAuth trust root handed to client apps. */
internal fun nodeConnection(connectionJson: JSONObject, oauthRootHex: String?): NodeConnection =
    NodeConnection(
        endpoint = connectionJson.getString("endpoint"),
        certificate = org.harmion.sdk.Hex.decode(connectionJson.getString("certificate")),
        nodeId = connectionJson.getString("node_id"),
        clusterId = connectionJson.getString("cluster_id"),
        oauthOrigin = connectionJson.optString("oauth_origin").ifEmpty { null },
        oauthRoot = oauthRootHex?.takeIf { it.isNotEmpty() }?.let(org.harmion.sdk.Hex::decode),
    )
