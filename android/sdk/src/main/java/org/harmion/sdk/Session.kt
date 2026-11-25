package org.harmion.sdk

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.withContext
import org.harmion.sdk.core.HarmionNative
import org.harmion.sdk.core.nativeReply
import org.json.JSONObject

/** One subscription notification, normalized for Kotlin callers. */
sealed interface SubscriptionEvent {
    data object SnapshotChunk : SubscriptionEvent

    data class SnapshotEnd(val contents: Map<String, JSONObject>) : SubscriptionEvent

    data class Changes(
        val upserted: List<String>,
        val removed: List<String>,
        val contents: Map<String, JSONObject>,
    ) : SubscriptionEvent

    data object Idle : SubscriptionEvent

    data object ResetRequired : SubscriptionEvent
}

data class SaveResult(val updateId: String, val view: JSONObject?)

data class QueryPage(val items: List<JSONObject>, val next: String?)

/**
 * Authorized client session for one node.
 *
 * [authorize] runs the existing integrated connection flow: the app starts a
 * request, the node's approval activity lets the owner review it, and the app
 * completes the PKCE exchange. Tokens live in Keystore-encrypted storage and are
 * refreshed without extending the owner-approved absolute expiry.
 */
class HarmionSession internal constructor(
    private val context: Context,
    internal val node: NodeClient,
    private val store: TokenStore,
) : AutoCloseable {
    private var handle: Long = 0L
    private var credentials: StoredCredentials? = null

    fun status(): NodeStatus = node.status()

    fun nodeSummary(): JSONObject = node.nodeSummary()

    /** True when a stored, non-expired authorization exists for this node. */
    fun isAuthorized(): Boolean {
        val stored = store.load() ?: return false
        val connection = runCatching { node.connectionInfo() }.getOrNull() ?: return false
        return stored.nodeId == connection.nodeId
    }

    suspend fun authorize(request: AuthorizationRequest, appName: String): Authorization {
        if (!withContext(Dispatchers.IO) { node.ensureRunning() }) {
            throw HarmionException("node_unavailable", "the Harmion node could not start")
        }
        val connection = awaitConnection()
        val verifier = Requests.pkceVerifier()
        val challenge = Requests.pkceChallenge(verifier)
        val state = Requests.randomIdHex(16)
        val offer = request.toOfferJson(appName, challenge, state)
        val anonymous =
            withContext(Dispatchers.IO) {
                nativeReply(HarmionNative.anonymous(connection.discoveryJson())) { it.getLong("handle") }
            }
        try {
            val started =
                withContext(Dispatchers.IO) {
                    nativeReply(HarmionNative.connectStart(anonymous, offer.toString())) {
                        it.getJSONObject("response")
                    }
                }
            val connectionId = started.getString("connection_id")
            val requestSecret = started.getString("request_secret")
            val token =
                withContext(Dispatchers.IO) { node.beginApproval(connectionId, requestSecret) }
            withContext(Dispatchers.Main) { node.launchApproval(connectionId, token) }

            val deadline = System.currentTimeMillis() + APPROVAL_TIMEOUT_MILLIS
            var completed: JSONObject? = null
            while (System.currentTimeMillis() < deadline) {
                val status =
                    withContext(Dispatchers.IO) {
                        nativeReply(
                            HarmionNative.connectStatus(
                                anonymous,
                                JSONObject()
                                    .put("connection_id", connectionId)
                                    .put("request_secret", requestSecret)
                                    .toString(),
                            ),
                        ) {
                            it.getJSONObject("response")
                        }
                    }
                when (status.optString("status")) {
                    "approved" -> {
                        completed =
                            withContext(Dispatchers.IO) {
                                nativeReply(
                                    HarmionNative.connectComplete(
                                        anonymous,
                                        JSONObject()
                                            .put("connection_id", connectionId)
                                            .put("request_secret", requestSecret)
                                            .put("code_verifier", verifier)
                                            .toString(),
                                    ),
                                ) {
                                    it.getJSONObject("response")
                                }
                            }
                        break
                    }
                    "denied" ->
                        throw HarmionException("denied", "the owner denied this request")
                    "expired" ->
                        throw HarmionException("expired", "the approval request expired")
                    "completed" ->
                        throw HarmionException("completed", "this request was already completed")
                    else -> delay(APPROVAL_POLL_MILLIS)
                }
            }
            val reply = completed ?: throw HarmionException("timeout", "approval timed out")
            val stored =
                StoredCredentials(
                    nodeId = connection.nodeId,
                    clusterId = connection.clusterId,
                    clientId = reply.getString("client_id"),
                    accessToken = reply.getString("access_token"),
                    refreshToken = reply.getString("refresh_token"),
                    expiresAtMillis =
                        System.currentTimeMillis() + reply.optLong("expires_in", 600L) * 1000L,
                    scope = reply.optString("scope"),
                )
            if (!withContext(Dispatchers.IO) {
                    node.registerCaller(connectionId, requestSecret)
                }
            ) {
                throw HarmionException(
                    "not_authorized",
                    "the node did not register this app as an approved caller",
                )
            }
            store.save(stored)
            invalidate()
            credentials = stored
            return Authorization(
                clientId = stored.clientId,
                accessToken = stored.accessToken,
                refreshToken = stored.refreshToken,
                expiresInSeconds = reply.optLong("expires_in", 600L),
                scope = stored.scope,
            )
        } finally {
            withContext(Dispatchers.IO) { HarmionNative.close(anonymous) }
        }
    }

    suspend fun save(requestJson: String): SaveResult =
        authed { handle ->
            val response =
                nativeReply(HarmionNative.save(handle, requestJson)) {
                    it.getJSONObject("response")
                }
            val view = response.optJSONObject("view")
            SaveResult(updateId = response.optString("update_id"), view = view)
        }

    suspend fun createContent(
        body: String,
        attributes: Map<String, Any?> = emptyMap(),
        kinds: List<String> = emptyList(),
        contentId: String = Requests.randomIdHex(),
        requestId: String = Requests.randomIdHex(),
        namespace: String = "app",
    ): SaveResult {
        val fields = mutableListOf(Requests.body(body))
        attributes.forEach { (key, value) -> fields += Requests.attribute(namespace, key, value) }
        kinds.forEach { fields += Requests.kind(it) }
        return save(Requests.create(contentId, fields, requestId))
    }

    suspend fun editContent(
        contentId: String,
        token: String,
        body: String? = null,
        attributes: Map<String, Any?> = emptyMap(),
        kinds: List<String> = emptyList(),
        requestId: String = Requests.randomIdHex(),
        namespace: String = "app",
    ): SaveResult {
        val fields = mutableListOf<JSONObject>()
        body?.let { fields += Requests.body(it) }
        attributes.forEach { (key, value) -> fields += Requests.attribute(namespace, key, value) }
        kinds.forEach { fields += Requests.kind(it) }
        require(fields.isNotEmpty()) { "edit needs at least one changed field" }
        return save(Requests.edit(contentId, token, fields, requestId))
    }

    suspend fun deleteContent(
        contentId: String,
        token: String,
        requestId: String = Requests.randomIdHex(),
    ): SaveResult = save(Requests.delete(contentId, token, requestId))

    suspend fun get(contentId: String): JSONObject =
        authed { handle ->
            nativeReply(HarmionNative.get(handle, contentId)) { it.getJSONObject("response") }
        }

    suspend fun query(condition: JSONObject, limit: Int = 32): QueryPage =
        authed { handle ->
            val response =
                nativeReply(HarmionNative.query(handle, condition.toString(), limit)) { it }
            val items =
                response.optJSONArray("items")?.let { array ->
                    (0 until array.length()).map { array.getJSONObject(it) }
                } ?: emptyList()
            QueryPage(items = items, next = response.optString("next").ifEmpty { null })
        }

    /** Owner-only historical replication confirmation. */
    suspend fun syncStatus(updateId: String? = null): JSONObject =
        authed { handle ->
            nativeReply(HarmionNative.syncStatus(handle, updateId.orEmpty())) {
                it.getJSONObject("response")
            }
        }

    /**
     * Live subscription. A reset (process restart, expired cursor or changed
     * grants) is reported and then re-subscribed after fetching a new snapshot.
     */
    fun observe(condition: JSONObject): Flow<SubscriptionEvent> =
        flow {
            var subscription = 0L
            try {
                while (true) {
                    val handle = ensureHandle()
                    if (subscription == 0L) {
                        subscription =
                            withContext(Dispatchers.IO) {
                                nativeReply(HarmionNative.subscribe(handle, condition.toString())) {
                                    it.getLong("subscription")
                                }
                            }
                    }
                    val event =
                        withContext(Dispatchers.IO) {
                            nativeReply(HarmionNative.subscriptionNext(handle, subscription)) { it }
                        }
                    when (val kind = event.getString("kind")) {
                        "snapshot_chunk" -> emit(SubscriptionEvent.SnapshotChunk)
                        "snapshot_end" ->
                            emit(SubscriptionEvent.SnapshotEnd(contents(event.optJSONObject("contents"))))
                        "changes" ->
                            emit(
                                SubscriptionEvent.Changes(
                                    upserted = strings(event, "upserted"),
                                    removed = strings(event, "removed"),
                                    contents = contents(event.optJSONObject("contents")),
                                ),
                            )
                        "idle" -> emit(SubscriptionEvent.Idle)
                        "reset_required" -> {
                            emit(SubscriptionEvent.ResetRequired)
                            withContext(Dispatchers.IO) {
                                runCatching { HarmionNative.unsubscribe(handle, subscription) }
                            }
                            subscription = 0L
                            delay(RESET_DELAY_MILLIS)
                        }
                        else -> throw HarmionException("invalid_response", "unknown event $kind")
                    }
                }
            } catch (error: HarmionException) {
                if (error.code == "http_401") {
                    refresh(force = true)
                    throw HarmionException("reauthorize", "authorization expired", error)
                }
                throw error
            } finally {
                if (subscription != 0L && handle != 0L) {
                    withContext(Dispatchers.IO) {
                        runCatching { HarmionNative.unsubscribe(handle, subscription) }
                    }
                }
            }
        }

    private fun strings(json: JSONObject, name: String): List<String> {
        val array = json.optJSONArray(name) ?: return emptyList()
        return (0 until array.length()).map { array.getString(it) }
    }

    private fun contents(json: JSONObject?): Map<String, JSONObject> {
        if (json == null) return emptyMap()
        val map = mutableMapOf<String, JSONObject>()
        for (key in json.keys()) {
            json.optJSONObject(key)?.let { map[key] = it }
        }
        return map
    }

    private val refreshMutex = Mutex()

    /** The runtime reports running slightly before discovery is published. */
    private suspend fun awaitConnection(): NodeConnection {
        var last: Throwable? = null
        repeat(6) {
            runCatching {
                    withContext(Dispatchers.IO) { node.connectionInfo() }
                }
                .onSuccess { return it }
                .onFailure { last = it }
            delay(600)
        }
        throw HarmionException(
            "node_unavailable",
            last?.message ?: "node discovery is not ready",
            last,
        )
    }

    private suspend fun <T> authed(block: (Long) -> T): T {
        if (!withContext(Dispatchers.IO) { node.ensureRunning() }) {
            throw HarmionException("node_unavailable", "the Harmion node could not start")
        }
        val first = ensureHandle()
        val used = credentials?.accessToken
        return try {
            withContext(Dispatchers.IO) { block(first) }
        } catch (error: HarmionException) {
            if (error.code != "http_401") throw error
            invalidate()
            if (used != null) refreshAfterFailure(used) else refresh(force = true)
            val second = ensureHandle()
            withContext(Dispatchers.IO) { block(second) }
        }
    }

    private suspend fun ensureHandle(): Long {
        if (handle != 0L) return handle
        val connection = awaitConnection()
        val stored =
            credentials ?: store.load()?.also { credentials = it }
                ?: throw HarmionException("not_authorized", "call authorize() first")
        if (stored.nodeId != connection.nodeId) {
            throw HarmionException("not_authorized", "stored authorization belongs to another node")
        }
        val active =
            if (System.currentTimeMillis() >= stored.expiresAtMillis - EXPIRY_SKEW_MILLIS) {
                refresh(force = true)
            } else {
                stored
            }
        handle =
            withContext(Dispatchers.IO) {
                nativeReply(HarmionNative.connect(connection.discoveryJson(), active.accessToken)) {
                    it.getLong("handle")
                }
            }
        return handle
    }

    private suspend fun refresh(force: Boolean = false): StoredCredentials =
        refreshMutex.withLock {
            val stored = credentials ?: store.load()
                ?: throw HarmionException("not_authorized", "no refresh token")
            if (!force && System.currentTimeMillis() < stored.expiresAtMillis - EXPIRY_SKEW_MILLIS) {
                return@withLock stored
            }
            refreshLocked(stored)
        }

    /**
     * Refreshes after a 401. Concurrent callers that failed with the same access
     * token share one refresh; a caller that lost the race reuses the winner's
     * token instead of consuming the same refresh token twice.
     */
    private suspend fun refreshAfterFailure(failedAccess: String): StoredCredentials =
        refreshMutex.withLock {
            val stored = credentials ?: store.load()
                ?: throw HarmionException("not_authorized", "no refresh token")
            if (stored.accessToken != failedAccess) return@withLock stored
            refreshLocked(stored)
        }

    private suspend fun refreshLocked(stored: StoredCredentials): StoredCredentials {
        val connection = awaitConnection()
        val origin = connection.oauthOrigin
            ?: throw HarmionException("not_authorized", "the node has no OAuth origin")
        val root = connection.oauthRoot
            ?: throw HarmionException("not_authorized", "the node has no OAuth trust root")
        val reply =
            withContext(Dispatchers.IO) {
                nativeReply(
                    HarmionNative.oauthRefresh(
                        origin,
                        root.toHex(),
                        stored.clientId,
                        stored.refreshToken,
                    ),
                ) { it }
            }
        val refreshed =
            stored.copy(
                accessToken = reply.getString("access_token"),
                refreshToken = reply.getString("refresh_token"),
                expiresAtMillis =
                    System.currentTimeMillis() + reply.optLong("expires_in", 600L) * 1000L,
                scope = reply.optString("scope", stored.scope),
            )
        store.save(refreshed)
        credentials = refreshed
        invalidate()
        return refreshed
    }

    private fun invalidate() {
        if (handle != 0L) {
            runCatching { HarmionNative.close(handle) }
            handle = 0L
        }
    }

    override fun close() {
        invalidate()
        node.close()
    }

    private companion object {
        const val APPROVAL_TIMEOUT_MILLIS = 5 * 60 * 1000L
        const val APPROVAL_POLL_MILLIS = 1000L
        const val EXPIRY_SKEW_MILLIS = 60 * 1000L
        const val RESET_DELAY_MILLIS = 1000L
    }
}

/** Entry point. */
object Harmion {
    /** Binds to the node app and returns a session bound to it. */
    suspend fun connect(context: Context): HarmionSession {
        val node = NodeClient.bind(context)
        return HarmionSession(context.applicationContext, node, TokenStore(context.applicationContext))
    }
}
