package org.harmion.mote

import android.content.Context
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import java.time.OffsetDateTime
import java.time.format.DateTimeFormatter

internal data class MotePreferences(
    val completionMotion: Boolean = true,
    val archiveHaptics: Boolean = true,
) {
    fun write(context: Context) {
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit()
            .putBoolean(COMPLETION_MOTION, completionMotion)
            .putBoolean(ARCHIVE_HAPTICS, archiveHaptics)
            .apply()
    }

    companion object {
        private const val FILE = "mote_preferences"
        private const val COMPLETION_MOTION = "completion_motion"
        private const val ARCHIVE_HAPTICS = "archive_haptics"

        fun read(context: Context): MotePreferences {
            val saved = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)
            return MotePreferences(
                completionMotion = saved.getBoolean(COMPLETION_MOTION, true),
                archiveHaptics = saved.getBoolean(ARCHIVE_HAPTICS, true),
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun MoteSettingsScreen(
    state: MoteUiState,
    preferences: MotePreferences,
    onBack: () -> Unit,
    onPreferences: (MotePreferences) -> Unit,
    onConnect: () -> Unit,
    onAuthorize: () -> Unit,
    onSync: () -> Unit,
    onOpenShared: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    var showAbout by rememberSaveable { mutableStateOf(false) }
    if (showAbout) {
        MoteAboutScreen(onBack = { showAbout = false })
        return
    }
    BackHandler(onBack = onBack)
    Scaffold(
        containerColor = colors.background,
        topBar = {
            TopAppBar(
                title = { Text("設定", Modifier.semantics { heading() }) },
                navigationIcon = {
                    IconButton(onClick = onBack) { MoteIcon(MoteGlyph.Back, "戻る") }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = colors.background),
            )
        },
    ) { insets ->
        Box(Modifier.fillMaxSize().padding(insets), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = 680.dp).fillMaxWidth().verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp, vertical = 12.dp),
                verticalArrangement = Arrangement.spacedBy(20.dp),
            ) {
                SettingsConnection(state.connection, state.busy, onConnect, onAuthorize)
                SettingsSync(state, onSync)
                Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.extraLarge, color = colors.surfaceContainer) {
                    Column {
                        SettingsToggle("アーカイブ時の触覚フィードバック", preferences.archiveHaptics) {
                            onPreferences(preferences.copy(archiveHaptics = it))
                        }
                    }
                }
                if (state.sharedAvailable && state.sharedDraft.isNotBlank()) {
                    Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.extraLarge, color = colors.surfaceContainer) {
                        Row(
                            Modifier.fillMaxWidth().clickable(enabled = !state.busy, onClick = onOpenShared)
                                .heightIn(min = 60.dp).padding(horizontal = 16.dp),
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            MoteIcon(MoteGlyph.Share, null, tint = colors.primary)
                            Text("共有の下書き", style = MaterialTheme.typography.bodyLarge)
                        }
                    }
                }
                OutlinedButton(onClick = { showAbout = true }, modifier = Modifier.fillMaxWidth()) {
                    MoteIcon(MoteGlyph.Info, null, Modifier.size(18.dp))
                    Text("Mote について", Modifier.padding(start = 8.dp))
                }
            }
        }
    }
}

@Composable
private fun MoteAboutScreen(onBack: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    BackHandler(onBack = onBack)
    Scaffold(
        containerColor = colors.background,
        topBar = {
            TopAppBar(
                title = { Text("Mote について", Modifier.semantics { heading() }) },
                navigationIcon = { IconButton(onClick = onBack) { MoteIcon(MoteGlyph.Back, "戻る") } },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = colors.background),
            )
        },
    ) { insets ->
        Box(Modifier.fillMaxSize().padding(insets), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = 680.dp).fillMaxWidth().verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp, vertical = 14.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                    Surface(shape = MaterialTheme.shapes.extraLarge, color = colors.primaryContainer, contentColor = colors.onPrimaryContainer) {
                        MoteMark(Modifier.padding(18.dp).size(36.dp), colors.onPrimaryContainer)
                    }
                    Column {
                        Text("Mote", style = MaterialTheme.typography.headlineMedium, fontWeight = androidx.compose.ui.text.font.FontWeight.Bold)
                        Text("Harmion のメモ", style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
                    }
                }
                AboutCard("バージョン") {
                    AboutInfo("アプリ", BuildConfig.VERSION_NAME, emphasis = true)
                    AboutInfo("ビルド", if (BuildConfig.OFFICIAL_RELEASE) "公式リリース" else "開発ビルド")
                    AboutInfo("種別", BuildConfig.BUILD_TYPE)
                    AboutInfo("ビルド日時", aboutBuildTime(BuildConfig.BUILD_TIME))
                    AboutInfo("パッケージ", BuildConfig.APPLICATION_ID)
                }
                AboutCard("リビジョン") {
                    AboutInfo("コミット", BuildConfig.GIT_REVISION + if (BuildConfig.GIT_DIRTY) " (変更あり)" else "", emphasis = true)
                    AboutInfo("ブランチ", BuildConfig.GIT_BRANCH)
                }
            }
        }
    }
}

@Composable
private fun AboutCard(title: String, content: @Composable ColumnScope.() -> Unit) {
    Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.extraLarge, color = MaterialTheme.colorScheme.surfaceContainer) {
        Column(Modifier.padding(horizontal = 16.dp, vertical = 14.dp), verticalArrangement = Arrangement.spacedBy(7.dp)) {
            Text(title, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            content()
        }
    }
}

@Composable
private fun AboutInfo(label: String, value: String, emphasis: Boolean = false) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.Top) {
        Text(label, Modifier.widthIn(min = 84.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(
            value,
            Modifier.weight(1f),
            style = MaterialTheme.typography.bodyMedium,
            textAlign = androidx.compose.ui.text.style.TextAlign.End,
            fontWeight = if (emphasis) androidx.compose.ui.text.font.FontWeight.SemiBold else null,
        )
    }
}

private fun aboutBuildTime(raw: String): String = runCatching {
    OffsetDateTime.parse(raw).format(DateTimeFormatter.ofPattern("yyyy/M/d HH:mm"))
}.getOrDefault(raw)

@Composable
private fun SettingsToggle(label: String, checked: Boolean, onCheckedChange: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().toggleable(checked, role = Role.Switch, onValueChange = onCheckedChange)
            .heightIn(min = 62.dp).padding(horizontal = 16.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
        Switch(checked = checked, onCheckedChange = null)
    }
}

@Composable
private fun SettingsConnection(connection: MoteConnection, busy: Boolean, onConnect: () -> Unit, onAuthorize: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val label = when (connection) {
        MoteConnection.Ready -> "接続済み"
        MoteConnection.Loading -> "接続中"
        MoteConnection.NeedsApproval -> "承認が必要です"
        MoteConnection.Approving -> "承認待ち"
        MoteConnection.Unavailable, MoteConnection.Failed -> "未接続"
    }
    val action = when (connection) {
        MoteConnection.Ready -> "再読み込み"
        MoteConnection.NeedsApproval -> "承認する"
        MoteConnection.Unavailable, MoteConnection.Failed -> "再接続"
        MoteConnection.Loading -> "再接続"
        MoteConnection.Approving -> "承認をやり直す"
    }
    Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.extraLarge, color = colors.surfaceContainerHigh) {
        Row(
            Modifier.heightIn(min = 74.dp).padding(horizontal = 16.dp, vertical = 9.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            MoteIcon(MoteGlyph.Link, null, tint = colors.primary)
            Column(Modifier.weight(1f)) {
                Text("Harmion", style = MaterialTheme.typography.titleMedium)
                Text(label, color = colors.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
            }
            val retryable = connection == MoteConnection.Loading || connection == MoteConnection.Approving
            TextButton(
                enabled = retryable || !busy,
                onClick = if (connection == MoteConnection.NeedsApproval || connection == MoteConnection.Approving) onAuthorize else onConnect,
            ) {
                Text(action)
            }
        }
    }
}

@Composable
private fun SettingsSync(state: MoteUiState, onSync: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val sync = state.sync
    val syncing = state.syncRequestBusy || sync.inFlight
    val ready = state.connection == MoteConnection.Ready
    val status = when {
        !sync.enabled -> "自動同期はオフ"
        !sync.permitted -> "同期は保留中"
        syncing -> "同期中"
        sync.lastError != null -> "前回の同期に失敗"
        else -> "待機"
    }
    Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.extraLarge, color = colors.surfaceContainer) {
        Row(
            Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Surface(shape = CircleShape, color = colors.primaryContainer.copy(alpha = .8f)) {
                Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
                    MoteIcon(MoteGlyph.Sync, null, Modifier.size(19.dp), colors.primary)
                }
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text("同期", style = MaterialTheme.typography.titleMedium)
                Text(status, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
                sync.lastSuccessAtMicros?.let {
                    Text("最終成功 ${formatSyncTime(it)}", style = MaterialTheme.typography.labelSmall, color = colors.onSurfaceVariant)
                }
                (sync.blockedMessage ?: sync.lastError)?.let {
                    Text(it, style = MaterialTheme.typography.labelSmall, color = colors.tertiary)
                }
            }
            TextButton(onClick = onSync, enabled = ready && !syncing) {
                Text(if (syncing) "同期中" else "今すぐ")
            }
        }
    }
}


private fun formatSyncTime(micros: Long): String =
    java.time.Instant.ofEpochSecond(Math.floorDiv(micros, 1_000_000L))
        .atZone(java.time.ZoneId.systemDefault())
        .format(java.time.format.DateTimeFormatter.ofPattern("M/d HH:mm"))
