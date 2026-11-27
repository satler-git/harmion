package org.harmion.android.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager

/** Which networks may be used for automatic sync. */
enum class NetworkRule(val storage: String) {
    WIFI_ONLY("wifi_only"),
    UNMETERED("unmetered"),
    ANY("any"),
    ;

    companion object {
        fun from(value: String?): NetworkRule = entries.firstOrNull { it.storage == value } ?: UNMETERED
    }
}

/** When automatic sync should pause for battery state. */
enum class BatteryRule(val storage: String) {
    UNRESTRICTED("unrestricted"),
    PAUSE_LOW("pause_low"),
    CHARGING_ONLY("charging_only"),
    ;

    companion object {
        fun from(value: String?): BatteryRule =
            entries.firstOrNull { it.storage == value } ?: PAUSE_LOW
    }
}

data class SyncSettings(
    val autoSync: Boolean = true,
    val network: NetworkRule = NetworkRule.UNMETERED,
    val battery: BatteryRule = BatteryRule.PAUSE_LOW,
    val allowRoaming: Boolean = false,
    val intervalMinutes: Int = 30,
    val onLocalSave: Boolean = true,
    val onAppConnect: Boolean = true,
    val onNetworkReturn: Boolean = true,
) {
    /** Runtime scheduler interval; 0 disables the in-process scheduler. */
    fun syncSeconds(): Long =
        if (!autoSync) 0L else (intervalMinutes.coerceIn(MIN_INTERVAL, MAX_INTERVAL) * 60L)

    companion object {
        const val MIN_INTERVAL = 15
        const val MAX_INTERVAL = 60
        val INTERVALS = listOf(15, 30, 60)
    }
}

class SyncPolicyStore(context: Context) {
    private val preferences =
        context.applicationContext.getSharedPreferences("org.harmion.android.sync", Context.MODE_PRIVATE)

    fun load(): SyncSettings =
        SyncSettings(
            autoSync = preferences.getBoolean("auto_sync", true),
            network = NetworkRule.from(preferences.getString("network", null)),
            battery = BatteryRule.from(preferences.getString("battery", null)),
            allowRoaming = preferences.getBoolean("roaming", false),
            intervalMinutes = preferences.getInt("interval", 30),
            onLocalSave = preferences.getBoolean("on_local_save", true),
            onAppConnect = preferences.getBoolean("on_app_connect", true),
            onNetworkReturn = preferences.getBoolean("on_network_return", true),
        )

    fun save(settings: SyncSettings) {
        preferences
            .edit()
            .putBoolean("auto_sync", settings.autoSync)
            .putString("network", settings.network.storage)
            .putString("battery", settings.battery.storage)
            .putBoolean("roaming", settings.allowRoaming)
            .putInt("interval", settings.intervalMinutes)
            .putBoolean("on_local_save", settings.onLocalSave)
            .putBoolean("on_app_connect", settings.onAppConnect)
            .putBoolean("on_network_return", settings.onNetworkReturn)
            .apply()
    }
}

/**
 * Stable codes shared by [PolicyEvaluator], the runtime status view and the UI.
 * The runtime only transports these strings; every user-facing sentence lives
 * in the UI layer so both stay in sync.
 */
object SyncBlockCode {
    const val AUTO_SYNC_DISABLED = "auto_sync_disabled"
    const val OFFLINE = "offline"
    const val ROAMING = "roaming"
    const val WIFI_ONLY = "wifi_only"
    const val METERED = "metered"
    const val CHARGING_ONLY = "charging_only"
    const val LOW_BATTERY = "low_battery"
    const val STORAGE = "storage"
    const val NETWORK = "network"
    const val POLICY = "policy"
}

data class PolicyDecision(
    val permitted: Boolean,
    val code: String? = null,
    val message: String? = null,
) {
    companion object {
        val Allowed = PolicyDecision(true)
    }
}

/**
 * Evaluates the user's sync policy against the actual network path and battery
 * state. WorkManager constraints are only a wake-up optimization: this check is
 * authoritative and is also pushed into the runtime before every sync round.
 */
class PolicyEvaluator(
    context: Context,
    private val store: SyncPolicyStore,
) {
    private val application = context.applicationContext

    fun settings(): SyncSettings = store.load()

    fun evaluate(): PolicyDecision {
        val settings = store.load()
        if (!settings.autoSync) {
            return PolicyDecision(
                false,
                SyncBlockCode.AUTO_SYNC_DISABLED,
                "自動同期が無効です",
            )
        }
        val connectivity = application.getSystemService(ConnectivityManager::class.java)
        val network =
            connectivity.activeNetwork
                ?: return PolicyDecision(false, SyncBlockCode.OFFLINE, "ネットワークがありません")
        val capabilities =
            connectivity.getNetworkCapabilities(network)
                ?: return PolicyDecision(false, SyncBlockCode.OFFLINE, "ネットワークがありません")
        if (!capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) ||
            !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
        ) {
            return PolicyDecision(false, SyncBlockCode.OFFLINE, "ネットワークに接続していません")
        }
        if (!settings.allowRoaming &&
            !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_ROAMING)
        ) {
            return PolicyDecision(false, SyncBlockCode.ROAMING, "ローミング中のため同期しません")
        }
        when (settings.network) {
            NetworkRule.WIFI_ONLY ->
                if (!capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) {
                    return PolicyDecision(
                        false,
                        SyncBlockCode.WIFI_ONLY,
                        "Wi-Fi接続を待っています",
                    )
                }
            NetworkRule.UNMETERED ->
                if (!capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)) {
                    return PolicyDecision(
                        false,
                        SyncBlockCode.METERED,
                        "従量制回線のため同期しません",
                    )
                }
            NetworkRule.ANY -> Unit
        }
        val battery = application.getSystemService(BatteryManager::class.java)
        val charging = battery.isCharging
        val capacity = battery.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        when (settings.battery) {
            BatteryRule.CHARGING_ONLY ->
                if (!charging) {
                    return PolicyDecision(
                        false,
                        SyncBlockCode.CHARGING_ONLY,
                        "充電中のみ同期します",
                    )
                }
            BatteryRule.PAUSE_LOW ->
                // Sync remains allowed at exactly 15%; only values below the
                // threshold pause. A negative capacity means the platform did
                // not report a level, so it must not be treated as empty.
                if (!charging && capacity in 0 until LOW_BATTERY_PERCENT) {
                    return PolicyDecision(
                        false,
                        SyncBlockCode.LOW_BATTERY,
                        "バッテリー残量が15%未満です",
                    )
                }
            BatteryRule.UNRESTRICTED -> Unit
        }
        return PolicyDecision.Allowed
    }

    /** WorkManager constraints that approximate [evaluate]; the gate remains final. */
    fun requiresCharging(): Boolean = store.load().battery == BatteryRule.CHARGING_ONLY

    companion object {
        /** Lowest charge that still permits sync; pause strictly below it. */
        const val LOW_BATTERY_PERCENT = 15
    }
}
