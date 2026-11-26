package org.harmion.sdk

import android.content.Context
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
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

/** Non-secret identity of a node authorization. A new approval can change [clientId]. */
data class AuthorizationIdentity(val clusterId: String, val nodeId: String, val clientId: String)

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
    // One session owns this app's authorization. Separate sessions/processes
    // sharing TokenStore are not serialized by this mutex.
    private val sessionMutex = Mutex()
    // close() cannot wait on the coroutine mutex (authorize may need Main).
    private val handle = AtomicLong()
    private var handleConnection: NodeConnection? = null
    private var credentials: StoredCredentials? = null

    fun status(): NodeStatus = node.status()

    fun nodeSummary(): JSONObject = node.nodeSummary()

    /** Advisory check for stored authorization; guarded operations check again before I/O. */
    fun isAuthorized(): Boolean {
        val stored = store.load() ?: return false
        val connection = runCatching { node.connectionInfo() }.getOrNull() ?: return false
        return stored.identity().matches(connection)
    }

    /** Returns the current stored authorization's non-secret identity without refreshing it. */
    suspend fun authorizationIdentity(): AuthorizationIdentity =
        serialized {
            ensureRunning()
            checkedAuthorization().stored.identity()
        }

    suspend fun authorize(request: AuthorizationRequest, appName: String): Authorization =
        serialized {
            ensureRunning()
            val connection = awaitConnection()
            val verifier = Requests.pkceVerifier()
            val challenge = Requests.pkceChallenge(verifier)
            val state = Requests.randomIdHex(16)
            val offer = request.toOfferJson(appName, challenge, state)
            val anonymous =
                nativeReply(HarmionNative.anonymous(connection.discoveryJson())) { it.getLong("handle") }
            try {
                val started = nativeReply(HarmionNative.connectStart(anonymous, offer.toString())) {
                    it.getJSONObject("response")
                }
                val connectionId = started.getString("connection_id")
                val requestSecret = started.getString("request_secret")
                val token = node.beginApproval(connectionId, requestSecret)
                withContext(Dispatchers.Main) { node.launchApproval(connectionId, token) }

                val deadline = System.currentTimeMillis() + APPROVAL_TIMEOUT_MILLIS
                var completed: JSONObject? = null
                while (System.currentTimeMillis() < deadline) {
                    val status = nativeReply(
                        HarmionNative.connectStatus(
                            anonymous,
                            JSONObject()
                                .put("connection_id", connectionId)
                                .put("request_secret", requestSecret)
                                .toString(),
                        ),
                    ) { it.getJSONObject("response") }
                    when (status.optString("status")) {
                        "approved" -> {
                            completed = nativeReply(
                                HarmionNative.connectComplete(
                                    anonymous,
                                    JSONObject()
                                        .put("connection_id", connectionId)
                                        .put("request_secret", requestSecret)
                                        .put("code_verifier", verifier)
                                        .toString(),
                                ),
                            ) { it.getJSONObject("response") }
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
                if (!stored.identity().matches(awaitConnection())) {
                    throw HarmionException("not_authorized", "the node changed during approval")
                }
                if (!node.registerCaller(connectionId, requestSecret)) {
                    throw HarmionException(
                        "not_authorized",
                        "the node did not register this app as an approved caller",
                    )
                }
                installCredentials(stored)
                Authorization(
                    clientId = stored.clientId,
                    accessToken = stored.accessToken,
                    refreshToken = stored.refreshToken,
                    expiresInSeconds = reply.optLong("expires_in", 600L),
                    scope = stored.scope,
                )
            } finally {
                HarmionNative.close(anonymous)
            }
        }

    suspend fun save(requestJson: String): SaveResult = saveAuthed(requestJson, null)

    /** Saves only under [expected], including any retry after refreshing the access token. */
    suspend fun save(requestJson: String, expected: AuthorizationIdentity): SaveResult =
        saveAuthed(requestJson, expected)

    private suspend fun saveAuthed(requestJson: String, expected: AuthorizationIdentity?): SaveResult =
        authed(expected) { handle ->
            val response =
                nativeReply(HarmionNative.save(handle, requestJson)) {
                    it.getJSONObject("response")
                }
            val updateId = (response.opt("update_id") as? String)?.takeIf { it.isNotBlank() }
                ?: throw HarmionException("invalid_response", "save response has no update_id")
            SaveResult(updateId = updateId, view = response.optJSONObject("view"))
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

    suspend fun get(contentId: String): JSONObject = getAuthed(contentId, null)

    /** Reads only under [expected], including any retry after refreshing the access token. */
    suspend fun get(contentId: String, expected: AuthorizationIdentity): JSONObject =
        getAuthed(contentId, expected)

    private suspend fun getAuthed(contentId: String, expected: AuthorizationIdentity?): JSONObject =
        authed(expected) { handle ->
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
    fun observe(condition: JSONObject): Flow<SubscriptionEvent> = observeAuthed(condition, null)

    /** Observes only under [expected]. After a 401 refresh, collection fails with `reauthorize`. */
    fun observe(condition: JSONObject, expected: AuthorizationIdentity): Flow<SubscriptionEvent> =
        observeAuthed(condition, expected)

    private fun observeAuthed(
        condition: JSONObject,
        expected: AuthorizationIdentity?,
    ): Flow<SubscriptionEvent> = flow {
        val conditionJson = condition.toString()
        var identity = expected
        while (true) {
            var subscription: LiveSubscription? = null
            try {
                serialized {
                    ensureRunning()
                    val auth = readyAuthorization(identity)
                    identity = auth.stored.identity()
                    // Publish before leaving the IO context: cancellation may discard its result.
                    subscription = openSubscription(auth, conditionJson)
                }
                val live = checkNotNull(subscription)
                while (true) {
                    val event = try {
                        // The long poll must not hold sessionMutex or delay saves.
                        withContext(Dispatchers.IO) {
                            nativeReply(HarmionNative.subscriptionNext(live.handle, live.id)) { it }
                        }
                    } catch (error: HarmionException) {
                        if (error.code != "http_401") throw error
                        serialized<Nothing> { subscriptionExpired(live.identity, live.accessToken, error) }
                    }
                    // Don't publish a result from a poll spanning a change of authorization.
                    serialized { checkedAuthorization(live.identity) }
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
                            break
                        }
                        else -> throw HarmionException("invalid_response", "unknown event $kind")
                    }
                }
            } finally {
                subscription?.let { live ->
                    withContext(NonCancellable + Dispatchers.IO) {
                        try {
                            HarmionNative.unsubscribe(live.handle, live.id)
                        } finally {
                            HarmionNative.close(live.handle)
                        }
                    }
                }
            }
            delay(RESET_DELAY_MILLIS)
        }
    }

    /** Each subscription owns an independent native handle until its final unsubscribe. */
    private data class LiveSubscription(
        val handle: Long,
        val id: Long,
        val identity: AuthorizationIdentity,
        val accessToken: String,
    )

    private suspend fun openSubscription(auth: CheckedAuthorization, condition: String): LiveSubscription {
        currentCoroutineContext().ensureActive()
        val ownedHandle = connect(auth)
        try {
            val id = nativeReply(HarmionNative.subscribe(ownedHandle, condition)) {
                it.getLong("subscription")
            }
            return LiveSubscription(ownedHandle, id, auth.stored.identity(), auth.stored.accessToken)
        } catch (error: Throwable) {
            HarmionNative.close(ownedHandle)
            if (error is HarmionException && error.code == "http_401") {
                subscriptionExpired(auth.stored.identity(), auth.stored.accessToken, error)
            }
            throw error
        }
    }

    private suspend fun subscriptionExpired(
        identity: AuthorizationIdentity,
        failedAccess: String,
        error: HarmionException,
    ): Nothing {
        refreshAfterFailure(identity, failedAccess)
        throw HarmionException("reauthorize", "subscription authorization expired; reconnect", error)
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

    private suspend fun <T> serialized(block: suspend () -> T): T =
        withContext(Dispatchers.IO) { sessionMutex.withLock { block() } }

    private fun ensureRunning() {
        if (handle.get() == CLOSED_HANDLE || !node.ensureRunning()) {
            throw HarmionException("node_unavailable", "the Harmion node could not start")
        }
    }

    /** The runtime reports running slightly before discovery is published. */
    private suspend fun awaitConnection(): NodeConnection {
        var last: HarmionException? = null
        repeat(6) {
            if (handle.get() == CLOSED_HANDLE) {
                throw HarmionException("node_unavailable", "the Harmion session is closed")
            }
            try {
                return node.connectionInfo()
            } catch (error: HarmionException) {
                last = error
                delay(600)
            }
        }
        throw HarmionException(
            "node_unavailable",
            last?.message ?: "node discovery is not ready",
            last,
        )
    }

    private fun StoredCredentials.identity(): AuthorizationIdentity =
        AuthorizationIdentity(clusterId = clusterId, nodeId = nodeId, clientId = clientId)

    private fun AuthorizationIdentity.matches(connection: NodeConnection): Boolean =
        nodeId == connection.nodeId && clusterId == connection.clusterId

    private data class CheckedAuthorization(val connection: NodeConnection, val stored: StoredCredentials)

    // All authorization/handle transitions below run on IO with sessionMutex held.
    private suspend fun checkedAuthorization(expected: AuthorizationIdentity? = null): CheckedAuthorization {
        val connection = awaitConnection()
        val stored = checkedCredentials(expected)
        if (!stored.identity().matches(connection)) {
            throw HarmionException("not_authorized", "the authorization belongs to another node or cluster")
        }
        val active = credentials
        if (active != stored) {
            // Same identity with a different token: don't use a stale cached native handle.
            invalidate()
            credentials = stored
        }
        return CheckedAuthorization(connection, stored)
    }

    private fun checkedCredentials(expected: AuthorizationIdentity?): StoredCredentials {
        val stored = store.load() ?: throw HarmionException("not_authorized", "call authorize() first")
        val identity = stored.identity()
        val active = credentials
        if ((expected != null && expected != identity) || (active != null && active.identity() != identity)) {
            throw HarmionException("not_authorized", "the authorization identity changed")
        }
        return stored
    }

    private suspend fun readyAuthorization(expected: AuthorizationIdentity?): CheckedAuthorization {
        val auth = checkedAuthorization(expected)
        return if (System.currentTimeMillis() >= auth.stored.expiresAtMillis - EXPIRY_SKEW_MILLIS) {
            refresh(auth)
        } else {
            auth
        }
    }

    private suspend fun <T> authed(expected: AuthorizationIdentity? = null, block: (Long) -> T): T =
        serialized {
            ensureRunning()
            val first = readyAuthorization(expected)
            currentCoroutineContext().ensureActive()
            try {
                block(ensureHandle(first))
            } catch (error: HarmionException) {
                if (error.code != "http_401") throw error
                val second = refreshAfterFailure(first.stored.identity(), first.stored.accessToken)
                currentCoroutineContext().ensureActive()
                block(ensureHandle(second))
            }
        }

    private fun connect(auth: CheckedAuthorization): Long =
        nativeReply(HarmionNative.connect(auth.connection.discoveryJson(), auth.stored.accessToken)) {
            it.getLong("handle")
        }

    private fun ensureHandle(auth: CheckedAuthorization): Long {
        if (handleConnection != auth.connection) invalidate()
        val existing = handle.get()
        if (existing == CLOSED_HANDLE) {
            throw HarmionException("node_unavailable", "the Harmion session is closed")
        }
        if (existing != 0L) return existing
        val opened = connect(auth)
        if (!handle.compareAndSet(0L, opened)) {
            HarmionNative.close(opened)
            throw HarmionException("node_unavailable", "the Harmion session is closed")
        }
        handleConnection = auth.connection
        return opened
    }

    /** Reuse the winner's credentials when a subscription raced another refresh. */
    private suspend fun refreshAfterFailure(
        expected: AuthorizationIdentity,
        failedAccess: String,
    ): CheckedAuthorization {
        val auth = checkedAuthorization(expected)
        if (auth.stored.accessToken != failedAccess) return auth
        invalidate()
        return refresh(auth)
    }

    private suspend fun refresh(auth: CheckedAuthorization): CheckedAuthorization {
        currentCoroutineContext().ensureActive()
        // The server rotates the refresh token. Don't lose its replacement to cancellation.
        return withContext(NonCancellable) {
            val stored = auth.stored
            val connection = auth.connection
            val origin = connection.oauthOrigin
                ?: throw HarmionException("not_authorized", "the node has no OAuth origin")
            val root = connection.oauthRoot
                ?: throw HarmionException("not_authorized", "the node has no OAuth trust root")
            val reply = nativeReply(
                HarmionNative.oauthRefresh(origin, root.toHex(), stored.clientId, stored.refreshToken),
            ) { it }
            val current = checkedAuthorization(stored.identity())
            if (current.stored != stored) return@withContext current
            val refreshed = stored.copy(
                accessToken = reply.getString("access_token"),
                refreshToken = reply.getString("refresh_token"),
                expiresAtMillis =
                    System.currentTimeMillis() + reply.optLong("expires_in", 600L) * 1000L,
                scope = reply.optString("scope", stored.scope),
            )
            installCredentials(refreshed)
            current.copy(stored = checkedCredentials(stored.identity()))
        }
    }

    private fun installCredentials(stored: StoredCredentials) {
        store.save(stored)
        invalidate()
        credentials = stored
    }

    private fun invalidate() {
        val previous = handle.getAndUpdate { if (it == CLOSED_HANDLE) CLOSED_HANDLE else 0L }
        if (previous > 0L) HarmionNative.close(previous)
        handleConnection = null
    }

    override fun close() {
        val previous = handle.getAndSet(CLOSED_HANDLE)
        if (previous > 0L) HarmionNative.close(previous)
        node.close()
    }

    private companion object {
        const val CLOSED_HANDLE = -1L
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
