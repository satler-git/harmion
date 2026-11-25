package org.harmion.android.service

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.harmion.android.HarmionApplication
import org.harmion.android.R
import org.harmion.android.core.NodeRuntimeController
import org.harmion.android.core.RuntimeSettings
import org.harmion.android.core.RuntimeSnapshot
import org.harmion.android.ui.MainActivity
import androidx.glance.appwidget.updateAll
import org.harmion.android.widget.HarmionWidget

/**
 * Optional always-on mode.
 *
 * The node itself is demand-driven: the UI, a bound SDK client or a worker keeps
 * it alive. This service is the explicit opt-in that keeps it running while the
 * app is in the background, with a persistent low-priority notification. It is
 * never started implicitly; the user enables it in Settings.
 */
class KeepAliveService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private lateinit var controller: NodeRuntimeController

    override fun onCreate() {
        super.onCreate()
        controller = (application as HarmionApplication).runtime
        controller.acquire()
        val notification = notification("ノードを起動しています…")
        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            notification,
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
            } else {
                0
            },
        )
        scope.launch {
            if (!controller.ensureStarted()) {
                stopSelf()
                return@launch
            }
            controller.snapshot.collect { snapshot -> update(snapshot) }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        scope.cancel()
        controller.release()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun update(snapshot: RuntimeSnapshot?) {
        val state =
            when {
                snapshot?.running != true -> "停止中"
                snapshot.syncBlockedCode != null -> "同期待機"
                snapshot.inflight.isNotEmpty() -> "同期中"
                else -> "稼働中"
            }
        val peers = snapshot?.peers ?: 0
        RuntimeSettings.writeWidgetState(this, state, peers)
        scope.launch { HarmionWidget().updateAll(this@KeepAliveService) }
        val text = "$state ・ ピア $peers 台"
        val manager = getSystemService(NotificationManager::class.java)
        manager.notify(NOTIFICATION_ID, notification(text))
    }

    private fun notification(text: String): Notification {
        val open =
            PendingIntent.getActivity(
                this,
                0,
                Intent(this, MainActivity::class.java),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
        return NotificationCompat.Builder(this, HarmionApplication.CHANNEL_RUNTIME)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Harmion")
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    private companion object {
        const val NOTIFICATION_ID = 0x4841
    }
}
