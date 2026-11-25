package org.harmion.android.ui

import androidx.compose.foundation.background
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
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.CloudDone
import androidx.compose.material.icons.rounded.CloudOff
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.Group
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Storage
import androidx.compose.material.icons.rounded.Sync
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.harmion.android.core.NodeRuntimeController
import org.harmion.android.core.RuntimeSnapshot

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun HomeScreen(controller: NodeRuntimeController, snapshot: RuntimeSnapshot?) {
    val scope = rememberCoroutineScope()
    var syncing by remember { mutableStateOf(false) }
    var confirmSync by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }

    val running = snapshot?.running == true
    val blocked = snapshot?.syncBlockedCode
    val inFlight = snapshot?.inflight?.isNotEmpty() == true
    val statusTitle =
        when {
            !running -> "停止中"
            blocked != null -> "同期を待機中"
            inFlight -> "同期しています"
            else -> "待機中"
        }
    val statusBody =
        when {
            !running -> "アプリを開くと再開します"
            blocked != null -> syncBlockedLabel(blocked)
            inFlight -> "${snapshot?.inflight?.size ?: 0} 台と同期中"
            snapshot?.lastSuccessAtMicros != null -> "最終 ${formatMicros(snapshot.lastSuccessAtMicros)}"
            else -> null
        }

    LazyColumn(
        modifier = Modifier.fillMaxWidth(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item {
            Surface(
                shape = MaterialTheme.shapes.extraLarge,
                color = MaterialTheme.colorScheme.primaryContainer,
                contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Column(
                    modifier =
                        Modifier.background(
                            Brush.linearGradient(
                                listOf(
                                    MaterialTheme.colorScheme.primaryContainer,
                                    MaterialTheme.colorScheme.tertiaryContainer,
                                ),
                            ),
                        ).padding(24.dp),
                    verticalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        if (inFlight) {
                            LoadingIndicator(modifier = Modifier.size(34.dp))
                        } else {
                            Icon(
                                if (running) Icons.Rounded.CloudDone else Icons.Rounded.CloudOff,
                                contentDescription = null,
                                modifier = Modifier.size(34.dp),
                            )
                        }
                        Column {
                            Text(
                                shortId(snapshot?.clusterId),
                                style = MaterialTheme.typography.labelLarge,
                                color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.75f),
                            )
                            Text(
                                statusTitle,
                                style = MaterialTheme.typography.headlineSmall,
                                fontWeight = FontWeight.Bold,
                            )
                        }
                    }
                    statusBody?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.85f),
                        )
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        StatusPill(
                            text = membershipLabel(snapshot?.membership),
                            container = MaterialTheme.colorScheme.surface.copy(alpha = 0.45f),
                            contentColor = MaterialTheme.colorScheme.onSurface,
                        )
                        StatusPill(
                            text = "${snapshot?.peers ?: 0} ピア",
                            container = MaterialTheme.colorScheme.surface.copy(alpha = 0.45f),
                            contentColor = MaterialTheme.colorScheme.onSurface,
                            icon = Icons.Rounded.Group,
                        )
                        StatusPill(
                            text = formatBytes(snapshot?.databaseBytes ?: 0),
                            container = MaterialTheme.colorScheme.surface.copy(alpha = 0.45f),
                            contentColor = MaterialTheme.colorScheme.onSurface,
                            icon = Icons.Rounded.Storage,
                        )
                    }
                }
            }
        }

        item { ConflictsSection(controller) }

        if (blocked != null) {
            item {
                HarmionCard(containerColor = MaterialTheme.colorScheme.errorContainer) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Icon(Icons.Rounded.CloudOff, contentDescription = null)
                        Column {
                            Text("自動同期を停止中", style = MaterialTheme.typography.titleSmall)
                            Text(
                                syncBlockedLabel(blocked),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
            }
        }

        if (snapshot?.membership == "unconfirmed") {
            item {
                HarmionCard(containerColor = MaterialTheme.colorScheme.tertiaryContainer) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Icon(Icons.Rounded.ErrorOutline, contentDescription = null)
                        Column {
                            Text("参加の承認待ち", style = MaterialTheme.typography.titleSmall)
                            Text(
                                "既存ノードの所有者に承認を依頼してください。",
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
            }
        }

        item {
            HarmionCard {
                SectionHeader("同期")
                InfoRow("現在", if (inFlight) "同期中" else "待機")
                snapshot?.retrying?.let { if (it > 0) InfoRow("再試行待ち", "$it ピア") }
                snapshot?.lastSuccessAtMicros?.let { InfoRow("最終成功", formatMicros(it)) }
                snapshot?.lastError?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                Button(
                    onClick = {
                        if (snapshot?.syncPermitted == false) {
                            confirmSync = true
                        } else {
                            syncing = true
                            scope.launch {
                                runCatching { controller.syncNow() }
                                    .onSuccess { message = "同期しました" }
                                    .onFailure { message = "同期できませんでした: ${it.message}" }
                                syncing = false
                            }
                        }
                    },
                    enabled = !syncing && running,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    if (syncing) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(18.dp),
                            strokeWidth = 2.dp,
                            color = MaterialTheme.colorScheme.onPrimary,
                        )
                    } else {
                        Icon(Icons.Rounded.Refresh, contentDescription = null, modifier = Modifier.size(18.dp))
                    }
                    Spacer(Modifier.size(8.dp))
                    Text(if (syncing) "同期しています…" else "今すぐ同期")
                }
                message?.let {
                    Text(it, style = MaterialTheme.typography.bodySmall)
                }
            }
        }

        item {
            HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                SectionHeader("ノード")
                InfoRow("ノードID", shortId(snapshot?.nodeId), emphasis = true)
            }
        }

        item { Spacer(Modifier.height(8.dp)) }
    }

    if (confirmSync) {
        AlertDialog(
            onDismissRequest = { confirmSync = false },
            title = { Text("今回は同期ポリシーを無視しますか？") },
            text = {
                Text(
                    "${syncBlockedLabel(snapshot?.syncBlockedCode ?: "")}。今回だけ実行します。",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    confirmSync = false
                    syncing = true
                    scope.launch {
                        runCatching { controller.syncNow() }
                            .onSuccess { message = "同期しました" }
                            .onFailure { message = "同期できませんでした: ${it.message}" }
                        syncing = false
                    }
                }) { Text("今回だけ同期") }
            },
            dismissButton = {
                TextButton(onClick = { confirmSync = false }) { Text("キャンセル") }
            },
        )
    }
}
