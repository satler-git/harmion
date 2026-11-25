package org.harmion.android.service

import android.content.Context
import org.json.JSONObject

/**
 * Packages that completed the owner-approved connection flow.
 *
 * Binder identity alone only proves which package is calling; it does not prove
 * that the app was ever granted access. The approval activity records the
 * package and signing certificate here, and the service consults it before
 * exposing status, node summaries or sync hints. Certificate changes or package
 * reinstalls invalidate the entry; revocation removes it.
 */
object CallerAccess {
    private const val PREFS = "org.harmion.callers"
    private const val KEY = "entries"

    @Synchronized
    fun record(context: Context, packageName: String, certificateSha256: String, appId: String) {
        if (packageName.isEmpty() || certificateSha256.isEmpty()) return
        val entries = load(context)
        entries.put(
            packageName,
            JSONObject()
                .put("certificate", certificateSha256)
                .put("app_id", appId)
                .put("approved_at", System.currentTimeMillis()),
        )
        save(context, entries)
    }

    @Synchronized
    fun isAllowed(context: Context, packageName: String, certificateSha256: String): Boolean {
        val entry = load(context).optJSONObject(packageName) ?: return false
        return entry.optString("certificate") == certificateSha256
    }

    @Synchronized
    fun revokeApp(context: Context, appId: String) {
        if (appId.isEmpty()) return
        val entries = load(context)
        val remove = mutableListOf<String>()
        for (packageName in entries.keys()) {
            if (entries.optJSONObject(packageName)?.optString("app_id") == appId) {
                remove += packageName
            }
        }
        if (remove.isEmpty()) return
        remove.forEach(entries::remove)
        save(context, entries)
    }

    private fun load(context: Context): JSONObject {
        val raw =
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, null)
                ?: return JSONObject()
        return runCatching { JSONObject(raw) }.getOrDefault(JSONObject())
    }

    private fun save(context: Context, entries: JSONObject) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY, entries.toString())
            .apply()
    }
}
