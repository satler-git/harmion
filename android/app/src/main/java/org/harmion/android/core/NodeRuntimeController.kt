package org.harmion.android.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import java.io.File
import java.net.Inet4Address
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.harmion.sdk.HarmionAdminClient
import org.harmion.sdk.Hex
import org.harmion.sdk.NodeConnection
import org.json.JSONObject

/**
 * The single owner of the embedded node runtime.
 *
 * Activities, the bound service and WorkManager never open the database or the
 * core themselves. They acquire the controller, which starts one runtime on
 * demand and stops it after the last user has gone and an idle grace period has
 * elapsed. Every start/stop is serialized by a mutex.
 */
class NodeRuntimeController(
    context: Context,
    private val policy: PolicyEvaluator,
) {
    private val application = context.applicationContext
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val startMutex = Mutex()
    private val usersLock = Any()
    private var users = 0
    private var idleJob: Job? = null
    private var pollJob: Job? = null

    @Volatile
    private var handle: Long = 0L

    @Volatile
    private var policyDecision: PolicyDecision = PolicyDecision.Allowed

    private val policyStore = SyncPolicyStore(application)
    private val _snapshot = MutableStateFlow<RuntimeSnapshot?>(null)

    /** Latest status, polled while the runtime is running. */
    val snapshot: StateFlow<RuntimeSnapshot?> = _snapshot.asStateFlow()

    val nodeRoot: File = File(application.filesDir, "harmion/node")

    val isInitialized: Boolean
        get() = File(nodeRoot, "node.json").isFile

    val running: Boolean
        get() = handle != 0L

    fun acquire() {
        synchronized(usersLock) {
            users += 1
            idleJob?.cancel()
            idleJob = null
        }
    }

    fun release() {
        synchronized(usersLock) {
            users = (users - 1).coerceAtLeast(0)
            if (users == 0) {
                idleJob?.cancel()
                idleJob =
                    scope.launch {
                        delay(IDLE_STOP_MILLIS)
                        startMutex.withLock { stopLocked() }
                    }
            }
        }
    }

    /** Initializes the node directory. A no-op when it already exists. */
    suspend fun initialize(clusterId: String? = null, useIroh: Boolean = true): JSONObject =
        withContext(Dispatchers.IO) {
            nodeRoot.parentFile?.mkdirs()
            val settings = policyStore.load()
            val options =
                JSONObject()
                    .apply {
                        if (clusterId != null) put("cluster_id", clusterId)
                        put("use_iroh", useIroh)
                        put("public_lookup", useIroh)
                        put("sync_seconds", settings.syncSeconds())
                        put("app_listen", "127.0.0.1:0")
                        put("oauth_listen", "127.0.0.1:0")
                    }
                    .toString()
            native(HarmionNative.initialize(nodeRoot.path, options)).require()
        }

    suspend fun identifiers(): JSONObject =
        withContext(Dispatchers.IO) {
            native(HarmionNative.identifiers(nodeRoot.path)).require()
        }

    suspend fun ensureStarted(): Boolean =
        startMutex.withLock {
            if (handle != 0L) return@withLock true
            if (!isInitialized) return@withLock false
            stopAndWaitForNothing()
            val settings = policyStore.load()
            val decision = policy.evaluate()
            policyDecision = decision
            var attempt =
                runCatching {
                    native(
                        HarmionNative.start(
                            nodeRoot.path,
                            startOptions(settings, useIroh = true, decision = decision),
                        ),
                    )
                }.getOrNull()
            if (attempt == null || !attempt.ok) {
                // iroh needs UDP and the platform's interface APIs. When the node
                // cannot bind it, fall back to the HTTPS peer listener on the
                // current LAN address so local clusters still work.
                attempt =
                    runCatching {
                        native(
                            HarmionNative.start(
                                nodeRoot.path,
                                startOptions(
                                    settings,
                                    useIroh = false,
                                    peerListen = localPeerAddress(),
                                    decision = decision,
                                ),
                            ),
                        )
                    }.getOrNull()
            }
            if (attempt == null || !attempt.ok) {
                return@withLock false
            }
            handle = attempt.json.getLong("handle")
            applyPolicy(decision)
            startPolling()
            true
        }

    private fun stopAndWaitForNothing() {
        // Nothing to do yet; reserved for a restart path when settings change.
    }

    private suspend fun stopLocked() {
        val current = handle
        handle = 0L
        pollJob?.cancel()
        pollJob = null
        if (current != 0L) {
            withContext(Dispatchers.IO) { runCatching { native(HarmionNative.stop(current)) } }
        }
        _snapshot.value = null
    }

    /** Stops the runtime even while the UI is still open. */
    suspend fun stop() = startMutex.withLock { stopLocked() }

    suspend fun refreshStatus(): RuntimeSnapshot? =
        withContext(Dispatchers.IO) {
            val current = handle
            if (current == 0L) {
                _snapshot.value = null
                return@withContext null
            }
            val reply = runCatching { native(HarmionNative.status(current)) }.getOrNull()
                ?: return@withContext _snapshot.value
            if (!reply.ok) {
                if (reply.code == "unknown_handle") {
                    handle = 0L
                    pollJob?.cancel()
                }
                return@withContext _snapshot.value
            }
            RuntimeSnapshot.fromJson(reply.json).also { _snapshot.value = it }
        }

    /**
     * Manual sync. [force] is only set after the user explicitly confirmed
     * ignoring the scheduling policy for this one round; the policy gate is
     * temporarily opened and restored afterwards. Background paths never use it.
     */
    suspend fun syncNow(force: Boolean = false): JSONObject =
        withContext(Dispatchers.IO) {
            val current = ensureHandle() ?: throw NativeFailure("not_running", "node is not running")
            if (!force) {
                return@withContext native(HarmionNative.syncNow(current)).require()
            }
            applyPolicy(PolicyDecision.Allowed)
            try {
                native(HarmionNative.syncNow(current)).require()
            } finally {
                applyPolicy(policy.evaluate())
            }
        }

    /** Background sync that still obeys the host policy. */
    suspend fun syncGated(): JSONObject =
        withContext(Dispatchers.IO) {
            val current = ensureHandle() ?: throw NativeFailure("not_running", "node is not running")
            native(HarmionNative.syncGated(current)).require()
        }

    fun requestSync(trigger: String) {
        val current = handle
        if (current == 0L) return
        runCatching { native(HarmionNative.requestSync(current, trigger)) }
    }

    fun applyPolicy(decision: PolicyDecision) {
        policyDecision = decision
        val current = handle
        if (current == 0L) return
        val reason =
            if (decision.permitted) {
                ""
            } else {
                JSONObject()
                    .put("code", decision.code ?: "policy")
                    .put("message", decision.message ?: "")
                    .toString()
            }
        val reply =
            runCatching {
                native(HarmionNative.setSyncPolicy(current, decision.permitted, reason))
            }.getOrNull()
        if (reply == null || !reply.ok) {
            // Keep the previous gate; the scheduler re-checks before each round.
            android.util.Log.w("Harmion", "sync policy update failed: ${reply?.message ?: "bridge"}")
        }
    }

    /** Re-evaluates the policy and applies it to a running runtime. */
    fun refreshPolicy() {
        applyPolicy(policy.evaluate())
    }

    fun settings(): SyncSettings = policyStore.load()

    fun saveSettings(settings: SyncSettings) {
        policyStore.save(settings)
        refreshPolicy()
    }

    /** Raw status JSON used by the bound service; errors keep their code. */
    suspend fun statusJson(): JSONObject =
        withContext(Dispatchers.IO) {
            if (!isInitialized) {
                return@withContext JSONObject().put("ok", true).put("running", false)
                    .put("membership", "uninitialized")
            }
            val current = handle
            if (current == 0L) {
                return@withContext JSONObject().put("ok", true).put("running", false)
                    .put("membership", "stopped")
            }
            native(HarmionNative.status(current)).json
        }

    fun adminCredentialHex(): String =
        Hex.encode(File(nodeRoot, "admin.secret").readBytes())

    fun nodeConnection(): NodeConnection? =
        runCatching {
            val connection =
                native(HarmionNative.connection(nodeRoot.path)).require()
                    .getJSONObject("connection")
            val root =
                runCatching { native(HarmionNative.oauthRoot(nodeRoot.path)) }
                    .getOrNull()
                    ?.takeIf { it.ok }
                    ?.json
                    ?.optString("root")
            nodeConnection(connection, root)
        }.getOrNull()

    /** Derives the OpenSSH public key and fingerprint for an Ed25519 seed. */
    suspend fun ownerPublicKey(seedHex: String): JSONObject =
        withContext(Dispatchers.IO) { native(HarmionNative.ownerPublicKey(seedHex)).require() }

    /** Signs an owner statement (hex) and returns an armored SSHSIG. */
    suspend fun ownerSign(seedHex: String, statementHex: String): JSONObject =
        withContext(Dispatchers.IO) {
            native(HarmionNative.ownerSign(seedHex, statementHex)).require()
        }

    /**
     * Enrolls the first owner key through the running node. Enrollment is
     * host-only and requires the runtime to own the directory lock.
     */
    suspend fun enrollOwner(publicKey: String, label: String): JSONObject {
        acquire()
        try {
            if (!ensureStarted()) throw NativeFailure("not_running", "node is not running")
            return withContext(Dispatchers.IO) {
                native(HarmionNative.ownerEnroll(handle, publicKey, label)).require()
            }
        } finally {
            release()
        }
    }

    fun adminClient(): HarmionAdminClient {
        val connection = nodeConnection() ?: throw NativeFailure("not_initialized", "node is not initialized")
        return HarmionAdminClient.connect(connection, adminCredentialHex())
    }

    suspend fun shutdown() {
        scope.launch { runCatching { startMutex.withLock { stopLocked() } } }.join()
    }

    private suspend fun ensureHandle(): Long {
        if (handle != 0L) return handle
        if (!ensureStarted()) throw NativeFailure("not_running", "node is not running")
        return handle
    }

    private fun startPolling() {
        if (pollJob?.isActive == true) return
        pollJob =
            scope.launch {
                while (isActive && handle != 0L) {
                    runCatching { refreshStatus() }
                    delay(POLL_INTERVAL_MILLIS)
                }
            }
    }

    private fun startOptions(
        settings: SyncSettings,
        useIroh: Boolean,
        peerListen: String? = null,
        decision: PolicyDecision = PolicyDecision.Allowed,
    ): String =
        JSONObject()
            .apply {
                put("use_iroh", useIroh)
                put("public_lookup", useIroh)
                put("sync_seconds", settings.syncSeconds())
                put("oauth_listen", "127.0.0.1:0")
                put("worker_threads", 2)
                if (!decision.permitted) {
                    put("sync_blocked_code", decision.code ?: "policy")
                    decision.message?.let { put("sync_blocked_message", it) }
                }
                if (!useIroh) {
                    put("peer_listen", peerListen ?: "127.0.0.1:0")
                }
            }
            .toString()

    private fun localPeerAddress(): String {
        val connectivity = application.getSystemService(ConnectivityManager::class.java)
        val properties: LinkProperties? =
            connectivity.activeNetwork?.let(connectivity::getLinkProperties)
        val address =
            properties?.linkAddresses?.firstOrNull { it.address is Inet4Address }?.address
        return address?.hostAddress?.let { "$it:0" } ?: "127.0.0.1:0"
    }

    private companion object {
        const val IDLE_STOP_MILLIS = 30_000L
        const val POLL_INTERVAL_MILLIS = 2_000L
    }
}
