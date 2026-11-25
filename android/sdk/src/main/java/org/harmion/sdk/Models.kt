package org.harmion.sdk

import org.json.JSONObject

/** A structured failure from the node or the local bridge. */
class HarmionException(
    val code: String,
    override val message: String,
    cause: Throwable? = null,
) : Exception(message, cause)

/**
 * Discovery information for one Harmion node, provided by its bound service.
 *
 * `certificate` is the DER leaf certificate of the loopback App API. The SDK
 * pins it exactly; the OS trust store is not consulted.
 */
data class NodeConnection(
    val endpoint: String,
    val certificate: ByteArray,
    val nodeId: String,
    val clusterId: String,
    val oauthOrigin: String?,
    val oauthRoot: ByteArray?,
) {
    /** Discovery JSON accepted by the native bridge (`ConnectionInfo`). */
    internal fun discoveryJson(): String =
        JSONObject()
            .put("version", 1)
            .put("cluster_id", clusterId)
            .put("node_id", nodeId)
            .put("certificate", certificate.joinToString("") { "%02x".format(it) })
            .put("endpoint", endpoint)
            .apply { oauthOrigin?.let { put("oauth_origin", it) } }
            .toString()

    override fun equals(other: Any?): Boolean =
        other is NodeConnection &&
            endpoint == other.endpoint &&
            nodeId == other.nodeId &&
            clusterId == other.clusterId &&
            certificate.contentEquals(other.certificate)

    override fun hashCode(): Int =
        listOf(endpoint, nodeId, clusterId, certificate.contentHashCode()).hashCode()

    companion object {
        internal fun fromJson(json: JSONObject): NodeConnection {
            fun requireHex(name: String): String {
                val value = json.optString(name)
                if (value.isEmpty()) throw HarmionException("invalid_connection", "missing $name")
                return value
            }
            fun bytes(name: String): ByteArray? {
                val value = json.optString(name)
                if (value.isEmpty()) return null
                return value.hexToBytes()
            }
            return NodeConnection(
                endpoint = requireHex("endpoint"),
                certificate = bytes("certificate")
                    ?: throw HarmionException("invalid_connection", "missing certificate"),
                nodeId = requireHex("node_id"),
                clusterId = requireHex("cluster_id"),
                oauthOrigin = json.optString("oauth_origin").ifEmpty { null },
                oauthRoot = bytes("oauth_root"),
            )
        }
    }
}

/** Snapshot of the node and scheduler state. */
data class NodeStatus(
    val running: Boolean,
    val clusterId: String,
    val nodeId: String,
    val membership: String,
    val peers: Int,
    val syncEnabled: Boolean,
    val syncPermitted: Boolean,
    val syncBlockedCode: String?,
    val syncBlockedMessage: String?,
    val lastSuccessAtMicros: Long?,
    val lastError: String?,
) {
    companion object {
        internal fun fromJson(json: JSONObject): NodeStatus {
            val sync = json.optJSONObject("sync") ?: JSONObject()
            val blocked = sync.optJSONObject("blocked")
            return NodeStatus(
                running = json.optBoolean("running"),
                clusterId = json.optString("cluster_id"),
                nodeId = json.optString("node_id"),
                membership = json.optString("membership", "unknown"),
                peers = json.optInt("peers", 0),
                syncEnabled = sync.optBoolean("enabled"),
                syncPermitted = sync.optBoolean("permitted", true),
                syncBlockedCode = blocked?.optString("code")?.ifEmpty { null },
                syncBlockedMessage = blocked?.optString("message")?.ifEmpty { null },
                lastSuccessAtMicros = sync.optLongOrNull("last_success_at"),
                lastError =
                    if (sync.has("last_error") && !sync.isNull("last_error")) {
                        sync.optString("last_error").ifEmpty { null }
                    } else {
                        null
                    },
            )
        }
    }
}

/** Result of an approved connection. */
data class Authorization(
    val clientId: String,
    val accessToken: String,
    val refreshToken: String,
    val expiresInSeconds: Long,
    val scope: String,
)

internal fun JSONObject.optLongOrNull(name: String): Long? =
    if (has(name) && !isNull(name)) optLong(name) else null

internal fun String.hexToBytes(): ByteArray {
    require(length % 2 == 0) { "hex length must be even" }
    return ByteArray(length / 2) { index ->
        substring(index * 2, index * 2 + 2).toInt(16).toByte()
    }
}

internal fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }
