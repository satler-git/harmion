package org.harmion.android.ui

import java.text.NumberFormat
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import org.harmion.android.core.SyncBlockCode
import org.json.JSONObject

// Harmion always renders clock times as 24-hour values. Explicit patterns keep
// the screens stable even when the system locale prefers AM/PM, and the locale
// only affects the surrounding labels.
private val dateTimeFormatter =
    DateTimeFormatter.ofPattern("yyyy/MM/dd HH:mm", Locale.JAPAN)
private val dateFormatter = DateTimeFormatter.ofPattern("yyyy/MM/dd", Locale.JAPAN)
private val clockFormatter = DateTimeFormatter.ofPattern("HH:mm", Locale.JAPAN)

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

/** "3分前" style relative time, with a 24-hour clock for recent values. */
internal fun formatMicros(micros: Long): String {
    val date = Instant.ofEpochMilli(micros / 1000).atZone(ZoneId.systemDefault())
    val elapsed = System.currentTimeMillis() - date.toInstant().toEpochMilli()
    return when {
        elapsed < 0 -> dateTimeFormatter.format(date)
        elapsed < 60_000 -> "たった今"
        elapsed < 3_600_000 -> "${elapsed / 60_000}分前"
        elapsed < 86_400_000 -> "${elapsed / 3_600_000}時間前"
        elapsed < 7 * 86_400_000 -> "${elapsed / 86_400_000}日前"
        else -> dateFormatter.format(date)
    }
}

/** Relative time plus an explicit 24-hour clock for at-a-glance accuracy. */
internal fun formatMicrosWithClock(micros: Long): String {
    val date = Instant.ofEpochMilli(micros / 1000).atZone(ZoneId.systemDefault())
    val elapsed = System.currentTimeMillis() - date.toInstant().toEpochMilli()
    return when {
        elapsed < 0 -> dateTimeFormatter.format(date)
        elapsed < 60_000 -> "たった今（${clockMicros(micros)}）"
        elapsed < 86_400_000 -> "${formatMicros(micros)}（${clockMicros(micros)}）"
        else -> dateTimeFormatter.format(date)
    }
}

internal fun absoluteMicros(micros: Long): String =
    dateTimeFormatter.format(
        Instant.ofEpochMilli(micros / 1000).atZone(ZoneId.systemDefault()),
    )

internal fun clockMicros(micros: Long): String =
    clockFormatter.format(
        Instant.ofEpochMilli(micros / 1000).atZone(ZoneId.systemDefault()),
    )

internal fun membershipLabel(value: String?): String =
    when (value) {
        "active" -> "active"
        "unconfirmed" -> "unconfirmed"
        "retired" -> "retired"
        "conflict" -> "conflict"
        else -> "—"
    }

internal fun memberStateLabel(value: String): String =
    when (value) {
        "active" -> "active"
        "retired" -> "retired"
        "pending" -> "pending"
        "unconfirmed" -> "unconfirmed"
        "conflict" -> "conflict"
        else -> value.ifBlank { "不明" }
    }

internal fun syncBlockedLabel(code: String?): String =
    when (code) {
        SyncBlockCode.AUTO_SYNC_DISABLED -> "自動同期が無効"
        SyncBlockCode.OFFLINE -> "ネットワーク待ち"
        SyncBlockCode.ROAMING -> "ローミング中は停止"
        SyncBlockCode.WIFI_ONLY -> "Wi-Fi待ち"
        SyncBlockCode.METERED -> "従量制回線のため停止"
        SyncBlockCode.CHARGING_ONLY -> "充電待ち"
        SyncBlockCode.LOW_BATTERY -> "バッテリー残量低下"
        SyncBlockCode.STORAGE -> "空き容量不足"
        SyncBlockCode.NETWORK -> "ネットワーク待ち"
        SyncBlockCode.POLICY -> "ポリシーにより停止"
        else -> "停止中"
    }

/** Stable per-peer sync failure codes emitted by the runtime. */
internal fun syncFailureLabel(code: String?): String =
    when (code) {
        "timeout" -> "応答がありません"
        "capacity" -> "相手が混雑しています"
        "transport_error" -> "接続できません"
        "invalid_response" -> "応答が不正です"
        "record_rejected" -> "更新が拒否されました"
        "peer_retired" -> "retired peer"
        "membership_conflict" -> "membership conflict"
        "unauthenticated" -> "認証に失敗しました"
        "identity_mismatch" -> "ノードIDが一致しません"
        "storage_error" -> "保存領域のエラー"
        "sync_error" -> "同期エラー"
        else -> "同期エラー"
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
        raw.contains("sync blocked by policy:") ->
            "同期を停止中: ${syncBlockedLabel(raw.substringAfter("sync blocked by policy:").trim())}"
        raw.contains("sync_blocked") -> "同期ポリシーにより停止しています。"
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
            } ?: "指定した種類"
        else -> "不明"
    }
