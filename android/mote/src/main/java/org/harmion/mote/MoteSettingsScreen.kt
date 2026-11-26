package org.harmion.mote

import android.content.Context
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

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
    onOpenShared: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
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
                Column(verticalArrangement = Arrangement.spacedBy(7.dp)) {
                    Text("操作の反応", Modifier.padding(horizontal = 12.dp).semantics { heading() },
                        color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelMedium)
                    Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.extraLarge, color = colors.surfaceContainer) {
                        Column {
                            SettingsToggle("長文保存時のアニメーション", preferences.completionMotion) {
                                onPreferences(preferences.copy(completionMotion = it))
                            }
                            HorizontalDivider(Modifier.padding(horizontal = 16.dp), color = colors.outlineVariant)
                            SettingsToggle("アーカイブ時の触覚フィードバック", preferences.archiveHaptics) {
                                onPreferences(preferences.copy(archiveHaptics = it))
                            }
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
            }
        }
    }
}

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
        MoteConnection.Loading, MoteConnection.Approving -> null
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
            if (action == null) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
            } else {
                TextButton(enabled = !busy, onClick = if (connection == MoteConnection.NeedsApproval) onAuthorize else onConnect) {
                    Text(action)
                }
            }
        }
    }
}
