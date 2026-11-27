package org.harmion.android.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.togetherWith
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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.CloudDone
import androidx.compose.material.icons.rounded.CloudOff
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.Group
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Storage
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject
import org.harmion.android.core.NodeRuntimeController
import org.harmion.android.core.SyncBlockCode
import org.harmion.android.core.RuntimeSnapshot
import org.harmion.android.ui.theme.HarmionSuccess

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun HomeScreen(controller: NodeRuntimeController, snapshot: RuntimeSnapshot?) {
    val scope = rememberCoroutineScope()
    var phase by remember { mutableStateOf(SyncActionPhase.Idle) }
    var feedback by remember { mutableStateOf<String?>(null) }
    var feedbackIsError by remember { mutableStateOf(false) }
    var attempt by remember { mutableIntStateOf(0) }

    val running = snapshot?.running == true
    // sync.enabled in the status snapshot is fixed at runtime start. Use the
    // user setting for the switch state, and ignore a stale auto-sync block
    // while the setting is already on.
    val autoSyncEnabled = controller.settings().autoSync
    val blocked =
        snapshot?.syncBlockedCode?.takeUnless {
            it == SyncBlockCode.AUTO_SYNC_DISABLED && autoSyncEnabled
        }
    val autoSyncOff = !autoSyncEnabled
    val blockedReason =
        snapshot?.syncBlockedMessage?.takeIf { it.isNotBlank() } ?: syncBlockedLabel(blocked)
    val inFlight = snapshot?.inflight?.isNotEmpty() == true
    val statusTitle =
        when {
            !running -> "停止中"
            autoSyncOff -> "手動同期のみ"
            blocked != null -> "同期を待機中"
            inFlight -> "同期しています"
            else -> "待機中"
        }
    val statusBody =
        when {
            !running -> "アプリを開くと再開します"
            blocked != null && !autoSyncOff -> blockedReason
            inFlight -> "${snapshot.inflight.size} 台と同期中"
            snapshot.lastSuccessAtMicros != null -> "最終 ${formatMicrosWithClock(snapshot.lastSuccessAtMicros)}"
            else -> null
        }

    fun startSync() {
        val current = ++attempt
        phase = SyncActionPhase.Running
        feedback = null
        feedbackIsError = false
        scope.launch {
            val result = runCatching { controller.syncNow(force = true) }
            if (current != attempt) return@launch
            result
                .onSuccess { reply ->
                    val outcome = readSyncOutcome(reply)
                    when {
                        outcome.failed > 0 -> {
                            phase = SyncActionPhase.Idle
                            feedbackIsError = true
                            feedback =
                                if (outcome.synced == 0) {
                                    "同期できませんでした（${outcome.failed} 台で失敗）"
                                } else {
                                    "一部のピアと同期できませんでした" +
                                        "（${outcome.failed}/${outcome.synced + outcome.failed} 台）"
                                }
                        }
                        outcome.synced == 0 -> {
                            phase = SyncActionPhase.Idle
                            feedbackIsError = false
                            feedback = "同期対象のピアがいません。"
                        }
                        else -> {
                            phase = SyncActionPhase.Done
                            delay(SUCCESS_DISPLAY_MILLIS)
                            if (current == attempt) phase = SyncActionPhase.Idle
                        }
                    }
                }
                .onFailure {
                    phase = SyncActionPhase.Idle
                    feedbackIsError = true
                    feedback = "同期できませんでした: ${errorLabel(it)}"
                }
        }
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

        item { PendingJoinCard(controller) }

        item { ConflictsSection(controller) }

        if (autoSyncOff) {
            item {
                HarmionCard(containerColor = MaterialTheme.colorScheme.tertiaryContainer) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Icon(Icons.Rounded.CloudOff, contentDescription = null)
                        Text("自動同期はオフです", style = MaterialTheme.typography.titleSmall)
                    }
                }
            }
        } else if (blocked != null) {
            item {
                HarmionCard(containerColor = MaterialTheme.colorScheme.errorContainer) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Icon(Icons.Rounded.CloudOff, contentDescription = null)
                        Column(Modifier.weight(1f)) {
                            Text("自動同期を停止中", style = MaterialTheme.typography.titleSmall)
                            Text(blockedReason, style = MaterialTheme.typography.bodySmall)
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
                InfoRow("現在", if (inFlight) "同期中" else if (running) "待機" else "停止中")
                snapshot?.retrying?.let { if (it > 0) InfoRow("再試行待ち", "$it ピア") }
                snapshot?.lastSuccessAtMicros?.let {
                    InfoRow(
                        "最終成功",
                        formatMicrosWithClock(it),
                    )
                }
                snapshot?.lastError?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                SyncActionButton(
                    phase = phase,
                    enabled = running && phase != SyncActionPhase.Running,
                    onClick = { startSync() },
                )
                feedback?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color =
                            if (feedbackIsError) {
                                MaterialTheme.colorScheme.error
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                        modifier =
                            Modifier.semantics {
                                liveRegion = LiveRegionMode.Polite
                            },
                    )
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
}

private enum class SyncActionPhase {
    Idle,
    Running,
    Done,
}

/**
 * The sync action doubles as its own progress and result display: the button
 * turns green and shows a check for a moment after a manual sync succeeds, so
 * the result is visible without a separate snackbar.
 */
@Composable
private fun SyncActionButton(
    phase: SyncActionPhase,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val done = phase == SyncActionPhase.Done
    val containerColor by
        animateColorAsState(
            targetValue =
                if (done) HarmionSuccess.container else MaterialTheme.colorScheme.primary,
            animationSpec = tween(320),
            label = "syncButtonContainer",
        )
    val contentColor by
        animateColorAsState(
            targetValue =
                if (done) HarmionSuccess.onContainer else MaterialTheme.colorScheme.onPrimary,
            animationSpec = tween(320),
            label = "syncButtonContent",
        )
    val checkScale by
        animateFloatAsState(
            targetValue = if (done) 1f else 0.7f,
            animationSpec =
                spring(
                    dampingRatio = Spring.DampingRatioMediumBouncy,
                    stiffness = Spring.StiffnessMedium,
                ),
            label = "syncCheckScale",
        )
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier =
            Modifier.fillMaxWidth().semantics {
                liveRegion = LiveRegionMode.Polite
            },
        colors =
            ButtonDefaults.buttonColors(
                containerColor = containerColor,
                contentColor = contentColor,
            ),
    ) {
        AnimatedContent(
            targetState = phase,
            transitionSpec = {
                (fadeIn(tween(180)) + scaleIn(initialScale = 0.85f)) togetherWith
                    fadeOut(tween(120))
            },
            label = "syncButtonContent",
        ) { shown ->
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                when (shown) {
                    SyncActionPhase.Idle ->
                        Icon(
                            Icons.Rounded.Refresh,
                            contentDescription = null,
                            modifier = Modifier.size(18.dp),
                        )
                    SyncActionPhase.Running ->
                        CircularProgressIndicator(
                            modifier = Modifier.size(18.dp),
                            strokeWidth = 2.dp,
                            color = contentColor,
                        )
                    SyncActionPhase.Done ->
                        Icon(
                            Icons.Rounded.CheckCircle,
                            contentDescription = null,
                            modifier =
                                Modifier.size(20.dp)
                                    .graphicsLayer {
                                        scaleX = checkScale
                                        scaleY = checkScale
                                    },
                        )
                }
                Text(
                    when (shown) {
                        SyncActionPhase.Idle -> "今すぐ同期"
                        SyncActionPhase.Running -> "同期しています…"
                        SyncActionPhase.Done -> "同期しました"
                    },
                )
            }
        }
    }
}

private data class SyncOutcome(val synced: Int, val failed: Int)

/**
 * Manual sync returns a report per peer. A call that succeeds at the transport
 * level can still have failed every peer, so the UI must not show the green
 * success state without reading the report.
 */
private fun readSyncOutcome(reply: JSONObject): SyncOutcome {
    val peers = reply.optJSONObject("report")?.optJSONArray("peers") ?: return SyncOutcome(0, 0)
    var synced = 0
    var failed = 0
    for (index in 0 until peers.length()) {
        val peer = peers.optJSONObject(index) ?: continue
        when (peer.optString("error")) {
            "" -> synced++
            // Retired and conflicted peers are expected to be skipped by the
            // runtime; they are not a sync failure the user needs to fix.
            "membership" -> Unit
            else -> failed++
        }
    }
    return SyncOutcome(synced, failed)
}

private const val SUCCESS_DISPLAY_MILLIS = 2_800L
