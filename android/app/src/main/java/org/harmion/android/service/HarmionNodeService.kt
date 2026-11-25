package org.harmion.android.service

import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Binder
import android.os.IBinder
import java.security.MessageDigest
import kotlinx.coroutines.runBlocking
import org.harmion.android.HarmionApplication
import org.harmion.android.approval.ApprovalRegistry
import org.harmion.android.core.NodeRuntimeController
import org.harmion.sdk.Hex
import org.json.JSONObject

/**
 * Bound control-plane service.
 *
 * Third-party apps bind here to discover the pinned App API endpoint, start the
 * node on demand and register an approval request. Data operations stay on the
 * HTTPS App API with OAuth credentials.
 *
 * The service is exported without a signature permission on purpose: any app may
 * ask. Everything that grants access is still gated by the verified Binder UID
 * and the owner's approval activity. Long operations are rate-limited per UID so
 * one app cannot pin the node or flood the binder.
 */
class HarmionNodeService : Service() {
    private lateinit var controller: NodeRuntimeController
    private val binder = HarmionBinder()
    private val throttleLock = Any()
    private val startThrottle = mutableMapOf<Int, Long>()
    private val approvalThrottle = mutableMapOf<Int, Long>()
    private val syncThrottle = mutableMapOf<Int, Long>()
    private val discoveryThrottle = mutableMapOf<Int, Long>()
    private val statusThrottle = mutableMapOf<Int, Long>()
    private val callerRegistrationThrottle = mutableMapOf<Int, Long>()
    private val startGate = Any()
    private var lastGlobalStart = 0L

    override fun onCreate() {
        super.onCreate()
        controller = (application as HarmionApplication).runtime
    }

    // Binding no longer keeps the runtime alive: any app can bind, so a held
    // binding must not defeat the idle stop. The UI, keep-alive service and
    // workers own explicit references; SDK calls start the node on demand.
    override fun onBind(intent: Intent?): IBinder = binder

    override fun onUnbind(intent: Intent?): Boolean = false

    private inner class HarmionBinder : IHarmionNodeService.Stub() {
        override fun connectionInfo(): String {
            val uid = Binder.getCallingUid()
            if (!admit(discoveryThrottle, uid, DISCOVERY_THROTTLE_MILLIS)) {
                return failure("rate_limited", "too many discovery requests")
            }
            if (!controller.isInitialized) {
                return failure("not_initialized", "the node is not initialized yet")
            }
            val connection =
                controller.nodeConnection()
                    ?: return failure("not_running", "the node has no discovery information")
            val identity = callerIdentity(uid)
            val approved =
                identity != null &&
                    CallerAccess.isAllowed(
                        this@HarmionNodeService,
                        identity.packageName,
                        identity.certificateSha256,
                    )
            return JSONObject()
                .put("ok", true)
                .put("endpoint", connection.endpoint)
                .put("certificate", Hex.encode(connection.certificate))
                .put("node_id", connection.nodeId)
                .put("cluster_id", connection.clusterId)
                .apply {
                    // OAuth trust material is only useful after approval; keep it
                    // out of the unauthenticated discovery response.
                    if (approved) {
                        connection.oauthOrigin?.let { put("oauth_origin", it) }
                        connection.oauthRoot?.let { put("oauth_root", Hex.encode(it)) }
                    }
                }
                .toString()
        }

        override fun status(): String {
            requireApproved()
            if (!controller.isInitialized) {
                return JSONObject()
                    .put("ok", true)
                    .put("running", false)
                    .put("membership", "uninitialized")
                    .toString()
            }
            val uid = Binder.getCallingUid()
            if (!admit(statusThrottle, uid, STATUS_THROTTLE_MILLIS)) {
                throw SecurityException("status rate limit exceeded")
            }
            return runBlocking { controller.statusJson() }.toString()
        }

        override fun ensureRunning(): Boolean {
            if (controller.running) return true
            if (!controller.isInitialized) return false
            val uid = Binder.getCallingUid()
            val approved = isApprovedCaller(uid)
            // Re-evaluate Wi-Fi/battery policy before the runtime starts, so the
            // scheduler never opens with a stale "permitted" gate.
            controller.refreshPolicy()
            val window = if (approved) START_THROTTLE_MILLIS else BOOTSTRAP_START_THROTTLE_MILLIS
            if (!admit(startThrottle, uid, window)) return false
            synchronized(startGate) {
                val now = System.currentTimeMillis()
                if (now - lastGlobalStart < GLOBAL_START_GATE_MILLIS) return false
                lastGlobalStart = now
            }
            return runBlocking { controller.ensureStarted() }
        }

        override fun requestSync() {
            requireApproved()
            val uid = Binder.getCallingUid()
            if (!admit(syncThrottle, uid, SYNC_THROTTLE_MILLIS)) return
            controller.requestSync("app_connect")
        }

        override fun beginApproval(connectionId: String, requestSecret: String): String {
            val uid = Binder.getCallingUid()
            val identity = callerIdentity(uid)
                ?: return failure("unknown_caller", "the calling app could not be identified")
            if (!connectionId.matches(CONNECTION_ID_PATTERN) ||
                !requestSecret.matches(SECRET_PATTERN)
            ) {
                return failure("invalid_connection", "malformed connection credentials")
            }
            if (!admit(approvalThrottle, uid, APPROVAL_THROTTLE_MILLIS)) {
                return failure("rate_limited", "too many approval requests")
            }
            // The secret binds the approval token to the request that this app
            // created. A guessed connection id alone is not enough.
            val pending =
                runCatching {
                    runBlocking {
                        val admin = controller.adminClient()
                        try {
                            admin
                                .call(
                                    "connect-status",
                                    JSONObject()
                                        .put("connection_id", connectionId)
                                        .put("request_secret", requestSecret)
                                        .toString()
                                        .toByteArray(Charsets.UTF_8),
                                )
                                .optString("status") == "pending"
                        } finally {
                            admin.close()
                        }
                    }
                }.getOrDefault(false)
            if (!pending) {
                return failure("unknown_connection", "no such pending connection")
            }
            val grant =
                ApprovalRegistry.register(
                    connectionId = connectionId,
                    uid = identity.uid,
                    packageName = identity.packageName,
                    certificateSha256 = identity.certificateSha256,
                )
                    ?: return failure("too_many_requests", "too many pending approvals")
            return JSONObject().put("ok", true).put("token", grant.token).toString()
        }

        override fun registerCaller(connectionId: String, requestSecret: String): Boolean {
            val uid = Binder.getCallingUid()
            val identity = callerIdentity(uid) ?: return false
            if (!connectionId.matches(CONNECTION_ID_PATTERN) ||
                !requestSecret.matches(SECRET_PATTERN)
            ) {
                return false
            }
            if (!admit(callerRegistrationThrottle, uid, CALLER_REGISTRATION_THROTTLE_MILLIS)) {
                return false
            }
            val verified =
                runCatching {
                    runBlocking {
                        val admin = controller.adminClient()
                        try {
                            val status =
                                admin
                                    .call(
                                        "connect-status",
                                        JSONObject()
                                            .put("connection_id", connectionId)
                                            .put("request_secret", requestSecret)
                                            .toString()
                                            .toByteArray(Charsets.UTF_8),
                                    )
                                    .optString("status")
                            if (status != "completed") return@runBlocking null
                            val appId =
                                admin.connections()
                                    .firstOrNull {
                                        it.optString("connection_id") == connectionId
                                    }
                                    ?.optString("app_id")
                                    .orEmpty()
                            appId.ifEmpty { null }
                        } finally {
                            admin.close()
                        }
                    }
                }.getOrNull()
            if (verified == null) return false
            CallerAccess.record(
                this@HarmionNodeService,
                identity.packageName,
                identity.certificateSha256,
                verified,
            )
            return true
        }

        override fun nodeSummary(): String {
            requireApproved()
            val identifiers = runCatching { runBlocking { controller.identifiers() } }.getOrNull()
            return JSONObject()
                .put("ok", true)
                .put("name", "Harmion")
                .put("initialized", controller.isInitialized)
                .put("running", controller.running)
                .put("node_id", identifiers?.optString("node_id").orEmpty())
                .put("cluster_id", identifiers?.optString("cluster_id").orEmpty())
                .put("founder", identifiers?.optBoolean("founder") ?: false)
                .toString()
        }
    }

    private data class CallerIdentity(
        val uid: Int,
        val packageName: String,
        val certificateSha256: String,
    )

    private fun isApprovedCaller(uid: Int): Boolean {
        val identity = callerIdentity(uid) ?: return false
        return CallerAccess.isAllowed(this, identity.packageName, identity.certificateSha256)
    }

    private fun requireApproved(): CallerIdentity {
        val identity = callerIdentity(Binder.getCallingUid())
        if (identity == null ||
            !CallerAccess.isAllowed(this, identity.packageName, identity.certificateSha256)
        ) {
            throw SecurityException("app is not approved")
        }
        return identity
    }

    /**
     * Package names and signing certificates are read from the package manager,
     * never from the caller's self-report. Multiple packages sharing one UID are
     * treated as one identity and the first owning package is recorded.
     */
    private fun callerIdentity(uid: Int): CallerIdentity? {
        val packages = packageManager.getPackagesForUid(uid) ?: return null
        for (name in packages) {
            val info =
                try {
                    packageManager.getPackageInfo(name, PackageManager.GET_SIGNING_CERTIFICATES)
                } catch (_: PackageManager.NameNotFoundException) {
                    continue
                }
            val signers = info.signingInfo?.apkContentsSigners ?: continue
            val signer = signers.firstOrNull() ?: continue
            val digest = MessageDigest.getInstance("SHA-256").digest(signer.toByteArray())
            return CallerIdentity(uid, name, Hex.encode(digest))
        }
        return null
    }

    /** Per-UID minimum interval. Guarded because binder calls arrive concurrently. */
    private fun admit(map: MutableMap<Int, Long>, uid: Int, windowMillis: Long): Boolean =
        synchronized(throttleLock) {
            val now = System.currentTimeMillis()
            val last = map[uid] ?: 0L
            if (now - last < windowMillis) {
                false
            } else {
                map[uid] = now
                true
            }
        }

    private fun failure(code: String, message: String): String =
        JSONObject()
            .put("ok", false)
            .put("error", JSONObject().put("code", code).put("message", message))
            .toString()

    private companion object {
        const val START_THROTTLE_MILLIS = 10_000L
        const val BOOTSTRAP_START_THROTTLE_MILLIS = 60_000L
        const val GLOBAL_START_GATE_MILLIS = 5_000L
        const val APPROVAL_THROTTLE_MILLIS = 1_000L
        const val SYNC_THROTTLE_MILLIS = 60_000L
        const val DISCOVERY_THROTTLE_MILLIS = 500L
        const val STATUS_THROTTLE_MILLIS = 1_000L
        const val CALLER_REGISTRATION_THROTTLE_MILLIS = 1_000L
        val CONNECTION_ID_PATTERN = Regex("^[0-9a-f]{64}$")
        val SECRET_PATTERN = Regex("^[0-9a-f]{64}$")
    }
}
