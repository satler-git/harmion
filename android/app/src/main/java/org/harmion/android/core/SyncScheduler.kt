package org.harmion.android.core

import android.content.Context
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.harmion.android.HarmionApplication

/**
 * WorkManager glue. It only decides *when* to wake the node; the runtime's policy
 * gate and durable sync cursor decide whether a round may run and where to resume.
 */
object SyncScheduler {
    const val PERIODIC_WORK = "harmion-periodic-sync"
    const val IMMEDIATE_WORK = "harmion-immediate-sync"

    fun configure(context: Context, settings: SyncSettings) {
        val manager = WorkManager.getInstance(context)
        if (!settings.autoSync) {
            manager.cancelUniqueWork(PERIODIC_WORK)
            return
        }
        val networkType =
            when {
                // Roaming is a hard rule; WorkManager can express one network
                // condition, so the stricter one wins.
                !settings.allowRoaming -> NetworkType.NOT_ROAMING
                settings.network == NetworkRule.ANY -> NetworkType.CONNECTED
                else -> NetworkType.UNMETERED
            }
        val constraints =
            Constraints.Builder()
                .setRequiredNetworkType(networkType)
                .setRequiresCharging(settings.battery == BatteryRule.CHARGING_ONLY)
                .build()
        val request =
            PeriodicWorkRequestBuilder<SyncWorker>(
                settings.intervalMinutes.coerceIn(SyncSettings.MIN_INTERVAL, SyncSettings.MAX_INTERVAL)
                    .toLong(),
                TimeUnit.MINUTES,
            )
                .setConstraints(constraints)
                .build()
        manager.enqueueUniquePeriodicWork(
            PERIODIC_WORK,
            ExistingPeriodicWorkPolicy.UPDATE,
            request,
        )
    }

    /** One-off pass, for example after the network came back. */
    fun requestImmediate(context: Context) {
        val request = OneTimeWorkRequestBuilder<SyncWorker>().build()
        WorkManager.getInstance(context).enqueueUniqueWork(
            IMMEDIATE_WORK,
            androidx.work.ExistingWorkPolicy.REPLACE,
            request,
        )
    }
}

class SyncWorker(
    context: Context,
    parameters: WorkerParameters,
) : CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        val application = applicationContext as HarmionApplication
        val controller = application.runtime
        if (!controller.isInitialized) return@withContext Result.success()
        val decision = application.policy.evaluate()
        controller.applyPolicy(decision)
        if (!decision.permitted) return@withContext Result.success()
        controller.acquire()
        try {
            if (!controller.ensureStarted()) return@withContext Result.retry()
            // Gate-respecting: a periodic wake-up must not override the policy.
            runCatching { controller.syncGated() }
            Result.success()
        } finally {
            controller.release()
        }
    }
}
