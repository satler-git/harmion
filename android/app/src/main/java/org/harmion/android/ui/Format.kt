package org.harmion.android.ui

import java.text.DateFormat
import java.text.NumberFormat
import java.util.Date
import org.json.JSONObject

internal fun shortId(value: String?): String {
    if (value.isNullOrEmpty()) return "—"
    return if (value.length <= 20) value else "${value.take(8)}…${value.takeLast(4)}"
}

internal fun formatBytes(bytes: Long): String {
    if (bytes < 1024) return "$bytes B"
    val units = listOf("KB", "MB", "GB", "TB")
    var value = bytes.toDouble() / 1024.0
    var index = 0
    while (value >= 1024 && index < units.lastIndex) {
        value /= 1024.0
        index++
    }
    return "${NumberFormat.getNumberInstance().apply { maximumFractionDigits = 1 }.format(value)} ${units[index]}"
}

/** "3分前" style relative time plus a full local date for accessibility. */
internal fun formatMicros(micros: Long): String {
    val date = Date(micros / 1000)
    val elapsed = System.currentTimeMillis() - date.time
    val relative =
        when {
            elapsed < 60_000 -> "たった今"
            elapsed < 3_600_000 -> "${elapsed / 60_000}分前"
            elapsed < 86_400_000 -> "${elapsed / 3_600_000}時間前"
            elapsed < 7 * 86_400_000 -> "${elapsed / 86_400_000}日前"
            else -> DateFormat.getDateInstance(DateFormat.MEDIUM).format(date)
        }
    return relative
}

internal fun absoluteMicros(micros: Long): String =
    DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(micros / 1000))

internal fun membershipLabel(value: String?): String =
    when (value) {
        "active" -> "承認済み"
        "unconfirmed" -> "承認待ち"
        "retired" -> "退役"
        "conflict" -> "競合"
        else -> "—"
    }

internal fun memberStateLabel(value: String): String =
    when (value) {
        "active" -> "稼働中"
        "retired" -> "退役済み"
        "pending" -> "保留中"
        "unconfirmed" -> "承認待ち"
        else -> value
    }

internal fun syncBlockedLabel(code: String): String =
    when (code) {
        "wifi_only" -> "Wi-Fi待ち"
        "unmetered" -> "非従量制回線待ち"
        "roaming" -> "ローミング中は停止"
        "battery_low" -> "バッテリー残量低下"
        "charging_only" -> "充電待ち"
        "network" -> "ネットワーク待ち"
        "storage" -> "空き容量不足"
        else -> "停止中"
    }

/**
 * User-facing text for admin/App API failures. The SDK reports transport
 * status as `Http(403)`; showing that verbatim tells the user nothing.
 */
internal fun errorLabel(error: Throwable?): String {
    val raw = error?.message?.trim().orEmpty()
    if (raw.isEmpty()) return "不明なエラーが発生しました。"
    return when {
        raw.contains("Http(400)") -> "要求を処理できませんでした。もう一度お試しください。"
        raw.contains("Http(401)") -> "認証に失敗しました。ノードを開き直してください。"
        raw.contains("Http(403)") -> "この端末に必要な管理権限がありません。"
        raw.contains("Http(404)") -> "対象が見つかりません。画面を更新してください。"
        raw.startsWith("Http(5") -> "ノードで一時的なエラーが発生しました。時間をおいて再試行してください。"
        raw.contains("timeout", ignoreCase = true) -> "ノードの応答がありません。再試行してください。"
        raw.contains("not_running") -> "ノードが停止しています。少し待ってから再試行してください。"
        raw.contains("OwnerRecoveryInvalid") || raw.contains("owner_recovery_invalid") ->
            "リカバリコードが違います。"
        raw.contains("OwnerAssertionRequired") || raw.contains("owner_assertion_required") ->
            "所有者の承認が必要です。"
        raw.contains("OwnerChallengeExpired") || raw.contains("owner_challenge_expired") ->
            "承認の有効期限が切れました。"
        raw.contains("cancelled", ignoreCase = true) -> "キャンセルしました。"
        else -> raw
    }
}

internal fun grantSummary(grants: JSONObject?): List<String> {
    if (grants == null) return emptyList()
    val summary = mutableListOf<String>()
    grants.optJSONObject("read")?.let { summary += "データの読み取り (${filterLabel(it)})" }
    grants.optJSONObject("write")?.let { summary += "データの書き込み (${filterLabel(it)})" }
    if (grants.optBoolean("call")) summary += "Interpreterの呼び出し"
    if (grants.optBoolean("provide")) summary += "Interpreterの提供"
    if (grants.optBoolean("manage_apps")) summary += "アプリ認可の管理"
    if (grants.optBoolean("manage_nodes")) summary += "ノード管理"
    return summary
}

private fun filterLabel(filter: JSONObject): String =
    when (filter.optString("type")) {
        "all" -> "すべて"
        "kinds" ->
            filter.optJSONArray("kinds")?.let { array ->
                (0 until array.length()).joinToString(", ") { array.getString(it) }
            } ?: "指定kind"
        else -> "不明"
    }
