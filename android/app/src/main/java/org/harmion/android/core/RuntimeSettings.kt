package org.harmion.android.core

import android.content.Context
import android.content.Intent
import androidx.core.content.ContextCompat
import org.harmion.android.service.KeepAliveService

/** Small process-wide preferences that are not part of the node configuration. */
object RuntimeSettings {
    private const val PREFS = "org.harmion.android.runtime"
    private const val KEY_KEEP_ALIVE = "keep_alive"
    private const val KEY_WIDGET_STATUS = "widget_status"
    private const val KEY_WIDGET_PEERS = "widget_peers"

    fun keepAlive(context: Context): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_KEEP_ALIVE, false)

    fun setKeepAlive(context: Context, enabled: Boolean) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putBoolean(KEY_KEEP_ALIVE, enabled)
            .apply()
    }

    fun startKeepAlive(context: Context) {
        ContextCompat.startForegroundService(
            context,
            Intent(context, KeepAliveService::class.java),
        )
    }

    fun stopKeepAlive(context: Context) {
        context.stopService(Intent(context, KeepAliveService::class.java))
    }

    /** Compact state for the home-screen widget; no identifiers or secrets. */
    data class WidgetState(val status: String, val peers: Int)

    fun widgetState(context: Context): WidgetState {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        return WidgetState(
            status = prefs.getString(KEY_WIDGET_STATUS, "未起動").orEmpty(),
            peers = prefs.getInt(KEY_WIDGET_PEERS, 0),
        )
    }

    fun writeWidgetState(context: Context, status: String, peers: Int) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY_WIDGET_STATUS, status)
            .putInt(KEY_WIDGET_PEERS, peers)
            .apply()
    }
}
