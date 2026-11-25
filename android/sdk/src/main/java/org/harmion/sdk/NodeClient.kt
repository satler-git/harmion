package org.harmion.sdk

import android.app.ActivityOptions
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Build
import android.os.IBinder
import java.security.MessageDigest
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.suspendCancellableCoroutine
import org.harmion.android.service.IHarmionNodeService
import org.json.JSONObject

/**
 * Control-plane connection to the Harmion node app.
 *
 * Holds no data: the pinned HTTPS App API is the data plane. The bound service
 * only starts the node on demand, publishes discovery information and registers
 * approval requests for the verified calling app.
 */
class NodeClient private constructor(
    private val context: Context,
    private val service: IHarmionNodeService,
    private val connection: ServiceConnection,
) : AutoCloseable {
    @Volatile
    private var closed = false

    fun connectionInfo(): NodeConnection =
        NodeConnection.fromJson(JSONObject(call { service.connectionInfo() }))

    fun status(): NodeStatus = NodeStatus.fromJson(JSONObject(call { service.status() }))

    fun ensureRunning(): Boolean = call { service.ensureRunning() }

    fun requestSync() {
        call { service.requestSync() }
    }

    fun nodeSummary(): JSONObject = JSONObject(call { service.nodeSummary() })

    internal fun registerCaller(connectionId: String, requestSecret: String): Boolean =
        call { service.registerCaller(connectionId, requestSecret) }

    /**
     * Registers an approval request for this app. The node verifies the Binder
     * UID, package and signing certificate before issuing a single-use token.
     */
    internal fun beginApproval(connectionId: String, requestSecret: String): String {
        val reply = JSONObject(call { service.beginApproval(connectionId, requestSecret) })
        if (!reply.optBoolean("ok")) {
            val error = reply.optJSONObject("error")
            throw HarmionException(
                error?.optString("code").orEmpty().ifEmpty { "approval_denied" },
                error?.optString("message").orEmpty(),
            )
        }
        return reply.getString("token")
    }

    internal fun launchApproval(connectionId: String, token: String) {
        val intent =
            Intent(ACTION_APPROVE)
                .setClassName(NODE_PACKAGE, APPROVAL_ACTIVITY)
                .putExtra(EXTRA_CONNECTION_ID, connectionId)
                .putExtra(EXTRA_APPROVAL_TOKEN, token)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        // Android 14+ hides the launching package unless the caller opts in.
        // The approval activity verifies getLaunchedFromPackage(), so without
        // this option every external approval would fail closed.
        val options =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                ActivityOptions.makeBasic().setShareIdentityEnabled(true).toBundle()
            } else {
                null
            }
        context.startActivity(intent, options)
    }

    private fun <T> call(block: IHarmionNodeService.() -> T): T {
        if (closed) throw HarmionException("node_unavailable", "node client is closed")
        return try {
            service.block()
        } catch (error: HarmionException) {
            throw error
        } catch (error: Exception) {
            throw HarmionException("node_unavailable", error.message ?: "node is unavailable", error)
        }
    }

    override fun close() {
        if (closed) return
        closed = true
        runCatching { context.unbindService(connection) }
    }

    companion object {
        /** Package of the node app. Also declared in the SDK's `<queries>`. */
        const val NODE_PACKAGE: String = "org.harmion.android"
        const val SERVICE_CLASS: String = "org.harmion.android.service.HarmionNodeService"
        const val APPROVAL_ACTIVITY: String = "org.harmion.android.approval.ApprovalActivity"
        const val ACTION_APPROVE: String = "org.harmion.android.action.APPROVE"
        const val EXTRA_CONNECTION_ID: String = "org.harmion.android.extra.CONNECTION_ID"
        const val EXTRA_APPROVAL_TOKEN: String = "org.harmion.android.extra.APPROVAL_TOKEN"

        private fun verifyNodeSignature(context: Context) {
            val info =
                try {
                    context.packageManager.getPackageInfo(
                        NODE_PACKAGE,
                        android.content.pm.PackageManager.GET_SIGNING_CERTIFICATES,
                    )
                } catch (_: android.content.pm.PackageManager.NameNotFoundException) {
                    throw HarmionException("node_unavailable", "Harmion node app is not installed")
                }
            val digests =
                info.signingInfo?.apkContentsSigners?.map {
                    Hex.encode(MessageDigest.getInstance("SHA-256").digest(it.toByteArray()))
                } ?: emptyList()
            if (!digests.contains(BuildConfig.NODE_CERT_SHA256)) {
                throw HarmionException(
                    "node_untrusted",
                    "the installed Harmion node app is not signed by the expected certificate",
                )
            }
        }

        suspend fun bind(context: Context): NodeClient = suspendCancellableCoroutine { continuation ->
            val appContext = context.applicationContext
            runCatching { verifyNodeSignature(appContext) }
                .onFailure {
                    continuation.resumeWithException(
                        it as? HarmionException
                            ?: HarmionException("node_untrusted", "cannot verify the node app", it),
                    )
                    return@suspendCancellableCoroutine
                }
            lateinit var connection: ServiceConnection
            connection =
                object : ServiceConnection {
                    override fun onServiceConnected(name: ComponentName, binder: IBinder) {
                        val service = IHarmionNodeService.Stub.asInterface(binder)
                        if (continuation.isActive) {
                            continuation.resume(NodeClient(appContext, service, this))
                        } else {
                            runCatching { appContext.unbindService(this) }
                        }
                    }

                    override fun onServiceDisconnected(name: ComponentName) {
                        // The node process may be reclaimed at any time; the caller
                        // discovers this on the next call and can re-bind.
                    }
                }
            val intent =
                Intent().setClassName(NODE_PACKAGE, SERVICE_CLASS)
            val bound =
                try {
                    appContext.bindService(intent, connection, Context.BIND_AUTO_CREATE)
                } catch (error: SecurityException) {
                    continuation.resumeWithException(
                        HarmionException("node_unavailable", "cannot bind to the Harmion node", error),
                    )
                    return@suspendCancellableCoroutine
                }
            if (!bound) {
                continuation.resumeWithException(
                    HarmionException("node_unavailable", "Harmion node app is not installed"),
                )
            }
            continuation.invokeOnCancellation { runCatching { appContext.unbindService(connection) } }
        }
    }
}
