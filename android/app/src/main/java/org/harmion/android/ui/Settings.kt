package org.harmion.android.ui

import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.BatteryChargingFull
import androidx.compose.material.icons.rounded.Fingerprint
import androidx.compose.material.icons.rounded.Info
import androidx.compose.material.icons.rounded.Key
import androidx.compose.material.icons.rounded.Notifications
import androidx.compose.material.icons.rounded.Power
import androidx.compose.material.icons.rounded.Schedule
import androidx.compose.material.icons.rounded.Shield
import androidx.compose.material.icons.rounded.Storage
import androidx.compose.material.icons.rounded.Sync
import androidx.compose.material.icons.rounded.Wifi
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.SecureFlagPolicy
import androidx.fragment.app.FragmentActivity
import java.security.SecureRandom
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.harmion.android.HarmionApplication
import org.harmion.android.core.BatteryRule
import org.harmion.android.core.NetworkRule
import org.harmion.android.core.NodeRuntimeController
import org.harmion.android.core.PolicyEvaluator
import org.harmion.android.core.RuntimeSettings
import org.harmion.android.core.SyncScheduler
import org.harmion.android.core.SyncSettings
import org.harmion.sdk.Hex
import org.json.JSONObject

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(controller: NodeRuntimeController, policy: PolicyEvaluator) {
    val context = LocalContext.current
    val application = context.applicationContext as HarmionApplication
    val scope = rememberCoroutineScope()
    var settings by remember { mutableStateOf(controller.settings()) }
    var saved by remember { mutableStateOf<String?>(null) }
    var identifiers by remember { mutableStateOf<JSONObject?>(null) }

    fun persist(updated: SyncSettings) {
        settings = updated
        controller.saveSettings(updated)
        SyncScheduler.configure(context, updated)
        saved = "保存しました"
    }

    var keepAlive by remember { mutableStateOf(RuntimeSettings.keepAlive(context)) }

    LaunchedEffect(Unit) {
        identifiers = runCatching { controller.identifiers() }.getOrNull()
    }

    LazyColumn(
        modifier = Modifier.fillMaxWidth(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item {
            HarmionCard {
                SectionHeader("同期ポリシー")
                SettingSwitch(
                    icon = Icons.Rounded.Sync,
                    title = "自動同期",
                    checked = settings.autoSync,
                    onCheckedChange = { persist(settings.copy(autoSync = it)) },
                )
                Divider()
                Text(
                    "ネットワーク",
                    style = MaterialTheme.typography.labelLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                SegmentedRow(
                    options =
                        listOf(
                            NetworkRule.WIFI_ONLY to "Wi-Fi",
                            NetworkRule.UNMETERED to "非従量制",
                            NetworkRule.ANY to "すべて",
                        ),
                    selected = settings.network,
                    onSelect = { persist(settings.copy(network = it)) },
                )
                Text(
                    "バッテリー",
                    style = MaterialTheme.typography.labelLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                SegmentedRow(
                    options =
                        listOf(
                            BatteryRule.UNRESTRICTED to "制限なし",
                            BatteryRule.PAUSE_LOW to "15%未満",
                            BatteryRule.CHARGING_ONLY to "充電中",
                        ),
                    selected = settings.battery,
                    onSelect = { persist(settings.copy(battery = it)) },
                )
                SettingSwitch(
                    icon = Icons.Rounded.Wifi,
                    title = "ローミング中も許可",
                    checked = settings.allowRoaming,
                    onCheckedChange = { persist(settings.copy(allowRoaming = it)) },
                )
                Text(
                    "定期同期の間隔",
                    style = MaterialTheme.typography.labelLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                SegmentedRow(
                    options = SyncSettings.INTERVALS.map { it to "${it}分" },
                    selected = settings.intervalMinutes,
                    onSelect = { persist(settings.copy(intervalMinutes = it)) },
                )
                Divider()
                SectionHeader("同期の開始条件")
                SettingSwitch(
                    icon = Icons.Rounded.Sync,
                    title = "ローカル更新時",
                    checked = settings.onLocalSave,
                    onCheckedChange = { persist(settings.copy(onLocalSave = it)) },
                )
                SettingSwitch(
                    icon = Icons.Rounded.Sync,
                    title = "アプリ接続時",
                    checked = settings.onAppConnect,
                    onCheckedChange = { persist(settings.copy(onAppConnect = it)) },
                )
                SettingSwitch(
                    icon = Icons.Rounded.Sync,
                    title = "ネットワーク復帰時",
                    checked = settings.onNetworkReturn,
                    onCheckedChange = { persist(settings.copy(onNetworkReturn = it)) },
                )
                saved?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                val evaluation = remember(settings) { policy.evaluate() }
                Text(
                    if (evaluation.permitted) {
                        "同期可能"
                    } else {
                        evaluation.message ?: "停止中"
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color =
                        if (evaluation.permitted) {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        } else {
                            MaterialTheme.colorScheme.error
                        },
                )
            }
        }

        item {
            HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                SectionHeader("バックグラウンド")
                SettingSwitch(
                    icon = Icons.Rounded.Power,
                    title = "常時稼働",
                    subtitle = "通知に状態を表示",
                    checked = keepAlive,
                    onCheckedChange = { enabled ->
                        keepAlive = enabled
                        RuntimeSettings.setKeepAlive(context, enabled)
                        if (enabled) {
                            RuntimeSettings.startKeepAlive(context)
                        } else {
                            RuntimeSettings.stopKeepAlive(context)
                        }
                    },
                )
            }
        }

        item { OwnerSection(controller) }

        item {
            HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                SectionHeader("ノード")
                identifiers?.let { ids ->
                    InfoRow("ノードID", shortId(ids.optString("node_id")), emphasis = true)
                    InfoRow("クラスタID", shortId(ids.optString("cluster_id")), emphasis = true)
                }

            }
        }

        item { Spacer(Modifier.height(8.dp)) }
    }
}

@Composable
private fun OwnerSection(controller: NodeRuntimeController) {
    val context = LocalContext.current
    val application = context.applicationContext as HarmionApplication
    val scope = rememberCoroutineScope()
    val ownerKeys = application.ownerKeys
    var ownerEnrolled by remember { mutableStateOf(ownerKeys.enrolled) }
    var ownerFingerprint by remember { mutableStateOf(ownerKeys.fingerprint) }
    var ownerEnforced by remember { mutableStateOf<Boolean?>(null) }
    var ownerBusy by remember { mutableStateOf(false) }
    var ownerError by remember { mutableStateOf<String?>(null) }
    var recoveryCode by remember { mutableStateOf<String?>(null) }
    var recoverOpen by remember { mutableStateOf(false) }
    var recoverInput by remember { mutableStateOf("") }
    var recoverError by remember { mutableStateOf<String?>(null) }
    var recoverBusy by remember { mutableStateOf(false) }
    val activity = context as? FragmentActivity

    LaunchedEffect(ownerEnrolled) {
        ownerEnforced =
            runCatching {
                withContext(Dispatchers.IO) {
                    val admin = controller.adminClient()
                    try {
                        admin.ownerStatus().optBoolean("enforce")
                    } finally {
                        admin.close()
                    }
                }
            }.getOrNull()
    }

    HarmionCard {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                SectionHeader("所有者承認")
            }
            Icon(
                Icons.Rounded.Fingerprint,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.primary,
            )
        }
        if (ownerEnrolled) {
            StatusPill(
                "登録済み",
                container = MaterialTheme.colorScheme.tertiaryContainer,
                contentColor = MaterialTheme.colorScheme.onTertiaryContainer,
            )
            ownerFingerprint?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            InfoRow(
                "適用",
                when (ownerEnforced) {
                    true -> "有効"
                    false -> "無効"
                    null -> "確認できません"
                },
            )
            Text(
                "リカバリコードは初回のみ表示",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            TextButton(onClick = { recoverOpen = true; recoverInput = ""; recoverError = null }) {
                Text("リカバリコードで再登録")
            }
        } else {
            ownerError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            TextButton(onClick = { recoverOpen = true; recoverInput = ""; recoverError = null }) {
                Text("リカバリコードで復元")
            }
            Button(
                enabled = activity != null && !ownerBusy,
                onClick = {
                    if (activity == null || ownerBusy) return@Button
                    ownerBusy = true
                    ownerError = null
                    scope.launch {
                        runCatching {
                            val seed = Hex.encode(ByteArray(32).also { SecureRandom().nextBytes(it) })
                            val key = controller.ownerPublicKey(seed)
                            val publicKey = key.getString("public_key")
                            val fingerprint = key.getString("fingerprint")
                            ownerKeys.create(activity, seed)
                            ownerKeys.storePublicKey(publicKey, fingerprint)
                            val enrollment = controller.enrollOwner(publicKey, "primary")
                            ownerKeys.markEnrolled()
                            ownerEnrolled = true
                            ownerFingerprint = fingerprint
                            recoveryCode = enrollment.getString("recovery_code")
                        }.onFailure {
                            ownerKeys.clear()
                            ownerError = it.message ?: "所有者キーを登録できませんでした"
                        }
                        ownerBusy = false
                    }
                },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Icon(Icons.Rounded.Fingerprint, contentDescription = null, modifier = Modifier.size(18.dp))
                Text(if (ownerBusy) "登録中…" else "所有者キーを登録", modifier = Modifier.padding(start = 8.dp))
            }
        }
    }

    if (recoverOpen) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { if (!recoverBusy) recoverOpen = false },
            title = { Text("リカバリコード") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        if (ownerEnrolled) "現在の所有者キーは失効します。" else "ノードの所有者キーを復元します。",
                        style = MaterialTheme.typography.bodySmall,
                    )
                    androidx.compose.material3.OutlinedTextField(
                        value = recoverInput,
                        onValueChange = { recoverInput = it.trim().take(64) },
                        singleLine = true,
                        label = { Text("コード") },
                        isError = recoverError != null,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    recoverError?.let {
                        Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                    }
                }
            },
            properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
            confirmButton = {
                TextButton(
                    enabled = !recoverBusy && recoverInput.length == 64 && activity != null,
                    onClick = {
                        if (activity == null) return@TextButton
                        recoverBusy = true
                        recoverError = null
                        scope.launch {
                            runCatching {
                                val seed = Hex.encode(ByteArray(32).also { SecureRandom().nextBytes(it) })
                                val key = controller.ownerPublicKey(seed)
                                val publicKey = key.getString("public_key")
                                val fingerprint = key.getString("fingerprint")
                                val recovered =
                                    withContext(Dispatchers.IO) {
                                        val admin = controller.adminClient()
                                        try {
                                            admin.recoverOwner(recoverInput, publicKey, "recovered")
                                        } finally {
                                            admin.close()
                                        }
                                    }
                                ownerKeys.create(activity, seed)
                                ownerKeys.storePublicKey(publicKey, fingerprint)
                                ownerKeys.markEnrolled()
                                ownerEnrolled = true
                                ownerFingerprint = fingerprint
                                recoverOpen = false
                                recoveryCode = recovered.getString("recovery_code")
                            }.onFailure { recoverError = it.message ?: "復元できませんでした" }
                            recoverBusy = false
                        }
                    },
                ) { Text(if (recoverBusy) "復元中…" else "復元") }
            },
            dismissButton = {
                TextButton(onClick = { recoverOpen = false }, enabled = !recoverBusy) { Text("キャンセル") }
            },
        )
    }

    recoveryCode?.let { code ->
        AlertDialog(
            onDismissRequest = { recoveryCode = null },
            title = { Text("リカバリコード") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("閉じると再表示できません")
                    Text(code, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Medium)
                }
            },
            properties = DialogProperties(securePolicy = SecureFlagPolicy.SecureOn),
            confirmButton = {
                TextButton(onClick = { recoveryCode = null }) { Text("保存しました") }
            },
        )
    }
}

@Composable
private fun SettingSwitch(
    icon: ImageVector,
    title: String,
    subtitle: String? = null,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(
            icon,
            contentDescription = null,
            modifier = Modifier.size(22.dp),
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            subtitle?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        Switch(checked = checked, onCheckedChange = onCheckedChange)
    }
}

@Composable
private fun <T> SegmentedRow(
    options: List<Pair<T, String>>,
    selected: T,
    onSelect: (T) -> Unit,
) {
    SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
        options.forEachIndexed { index, (value, label) ->
            SegmentedButton(
                selected = value == selected,
                onClick = { onSelect(value) },
                shape = SegmentedButtonDefaults.itemShape(index = index, count = options.size),
                label = { Text(label, maxLines = 1) },
            )
        }
    }
}

@Composable
private fun Divider() {
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
}
