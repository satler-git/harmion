package org.harmion.android.service

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import org.harmion.android.HarmionApplication
import org.harmion.android.R
import org.harmion.android.core.NodeRuntimeController
import org.harmion.android.ui.MainActivity
import org.json.JSONObject

/** Periodic, low-frequency check for conflicts and storage failures. */
class IssueCheckWorker(
    appContext: Context,
    params: WorkerParameters,
) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        val application = applicationContext as? HarmionApplication ?: return Result.success()
        val controller: NodeRuntimeController = application.runtime
        controller.acquire()
        return try {
            if (!controller.ensureStarted()) return Result.success()
            val conflicts = runCatching { countConflicts(controller) }.getOrDefault(0)
            val storage = storageProblem(controller.snapshot.value?.lastError)
            notify(conflicts, storage)
            Result.success()
        } catch (_: Throwable) {
            Result.retry()
        } finally {
            controller.release()
        }
    }

    private suspend fun countConflicts(controller: NodeRuntimeController): Int {
        val admin = controller.adminClient()
        return try {
            val page = admin.query("""{"type":"and","conditions":[]}""", 32)
            val items = page.optJSONArray("items") ?: return 0
            var count = 0
            for (index in 0 until items.length()) {
                val item = items.getJSONObject(index)
                if (item.optBoolean("deletion_conflict") ||
                    (item.optJSONArray("conflicts")?.length() ?: 0) > 0
                ) {
                    count += 1
                }
            }
            count
        } finally {
            admin.close()
        }
    }

    private fun storageProblem(error: String?): String? {
        val text = error?.lowercase() ?: return null
        return if (listOf("storage", "disk", "database", "corrupt", "quota", "full").any(text::contains)) {
            error
        } else {
            null
        }
    }

    private fun notify(conflicts: Int, storage: String?) {
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        if (conflicts > 0) {
            manager.notify(CONFLICT_ID, build("競合 $conflicts 件", "タップして解決"))
        } else {
            manager.cancel(CONFLICT_ID)
        }
        if (storage != null) {
            manager.notify(STORAGE_ID, build("保存に問題があります", storage.take(120)))
        } else {
            manager.cancel(STORAGE_ID)
        }
    }

    private fun build(title: String, text: String) =
        NotificationCompat.Builder(applicationContext, HarmionApplication.CHANNEL_ISSUES)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(
                PendingIntent.getActivity(
                    applicationContext,
                    0,
                    Intent(applicationContext, MainActivity::class.java),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                ),
            )
            .setAutoCancel(true)
            .build()

    private companion object {
        const val CONFLICT_ID = 0x4842
        const val STORAGE_ID = 0x4843
    }
}
