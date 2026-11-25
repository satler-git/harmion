package org.harmion.android

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.net.ConnectivityManager
import android.net.Network
import org.harmion.android.core.NodeRuntimeController
import org.harmion.android.core.PolicyEvaluator
import org.harmion.android.core.SyncPolicyStore
import org.harmion.android.core.RuntimeSettings
import org.harmion.android.core.SyncScheduler
import org.harmion.android.owner.OwnerAdmin
import org.harmion.android.owner.OwnerKeyStore

/**
 * Process-wide owner of the node runtime.
 *
 * Activities, the bound service and workers never initialize the core or the
 * database themselves: they go through [runtime], which owns the single runtime
 * instance for this process.
 */
class HarmionApplication : Application() {
    lateinit var policy: PolicyEvaluator
        private set

    lateinit var runtime: NodeRuntimeController
        private set

    lateinit var ownerKeys: OwnerKeyStore
        private set

    lateinit var ownerAdmin: OwnerAdmin
        private set

    private var networkCallback: ConnectivityManager.NetworkCallback? = null

    override fun onCreate() {
        super.onCreate()
        instance = this
        policy = PolicyEvaluator(this, SyncPolicyStore(this))
        runtime = NodeRuntimeController(this, policy)
        ownerKeys = OwnerKeyStore(this)
        ownerAdmin = OwnerAdmin(runtime, ownerKeys)
        createNotificationChannels()
        scheduleIssueChecks()
        if (RuntimeSettings.keepAlive(this)) {
            RuntimeSettings.startKeepAlive(this)
        }
        SyncScheduler.configure(this, policy.settings())
        registerNetworkCallback()
    }

    private fun createNotificationChannels() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_SYNC,
                getString(R.string.sync_channel_name),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_RUNTIME,
                getString(R.string.runtime_channel_name),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ISSUES,
                getString(R.string.issues_channel_name),
                NotificationManager.IMPORTANCE_DEFAULT,
            ),
        )
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_APPROVAL,
                getString(R.string.approval_channel_name),
                NotificationManager.IMPORTANCE_DEFAULT,
            ),
        )
    }

    private fun scheduleIssueChecks() {
        androidx.work.WorkManager.getInstance(this).enqueueUniquePeriodicWork(
            "harmion-issues",
            androidx.work.ExistingPeriodicWorkPolicy.KEEP,
            androidx.work.PeriodicWorkRequestBuilder<
                org.harmion.android.service.IssueCheckWorker
            >(30, java.util.concurrent.TimeUnit.MINUTES).build(),
        )
    }

    /**
     * A network return is a scheduling hint, not a reason to bypass the policy:
     * the runtime re-checks its gate before every round.
     */
    private fun registerNetworkCallback() {
        val connectivity = getSystemService(ConnectivityManager::class.java)
        val callback =
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    val decision = policy.evaluate()
                    runtime.applyPolicy(decision)
                    if (!decision.permitted) return
                    if (runtime.running) {
                        runtime.requestSync("network_return")
                    } else if (policy.settings().onNetworkReturn) {
                        SyncScheduler.requestImmediate(this@HarmionApplication)
                    }
                }

                override fun onLost(network: Network) {
                    runtime.applyPolicy(policy.evaluate())
                }
            }
        runCatching { connectivity.registerDefaultNetworkCallback(callback) }
            .onSuccess { networkCallback = callback }
    }

    companion object {
        const val CHANNEL_SYNC = "sync"
        const val CHANNEL_APPROVAL = "approval"
        const val CHANNEL_ISSUES = "issues"
        const val CHANNEL_RUNTIME = "runtime"

        lateinit var instance: HarmionApplication
            private set
    }
}
