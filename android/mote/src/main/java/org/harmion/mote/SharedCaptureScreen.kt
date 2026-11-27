package org.harmion.mote

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.net.URI

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SharedCaptureScreen(
    state: MoteUiState,
    onDraft: (String) -> Unit,
    onSend: () -> Unit,
    onCancel: () -> Unit,
    onReleasePending: () -> Unit,
    onConnect: () -> Unit,
    onAuthorize: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val pending = state.sharedPending
    val needsReview = pending && !state.busy
    var confirmRelease by remember { mutableStateOf(false) }
    if (confirmRelease && pending) {
        AlertDialog(
            onDismissRequest = { confirmRelease = false },
            text = { Text("下書きの編集を再開しますか？") },
            confirmButton = {
                TextButton(enabled = !state.busy, onClick = { confirmRelease = false; onReleasePending() }) { Text("編集を再開") }
            },
            dismissButton = { TextButton(onClick = { confirmRelease = false }) { Text("キャンセル") } },
        )
    }
    val canSend = state.sharedAvailable && state.sharedDraft.isNotBlank() &&
        state.connection == MoteConnection.Ready && !state.busy

    val link = remember(state.sharedDraft) { sharedLinkDraft(state.sharedDraft) }
    var editBody by rememberSaveable { mutableStateOf(false) }
    val readOnly = pending || state.busy

    BackHandler { if (!state.busy) onCancel() }
    Scaffold(
        modifier = Modifier.imePadding(),
        containerColor = colors.background,
        topBar = {
            TopAppBar(
                title = { Text("共有", Modifier.semantics { heading() }) },
                navigationIcon = {
                    IconButton(onClick = onCancel, enabled = !state.busy) { MoteIcon(MoteGlyph.Close, "閉じる") }
                },
                actions = {
                    if (link != null && editBody) {
                        IconButton(onClick = { editBody = false }) { MoteIcon(MoteGlyph.Check, "リンクをプレビュー") }
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = colors.background),
            )
        },
        bottomBar = {
            Surface(color = colors.background) {
                Box(
                    Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Bottom))
                        .padding(horizontal = 20.dp, vertical = 8.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Column(Modifier.widthIn(max = 560.dp).fillMaxWidth()) {
                        if (needsReview) {
                            TextButton(
                                onClick = { confirmRelease = true },
                                enabled = state.sharedAvailable,
                                modifier = Modifier.align(Alignment.End),
                            ) { Text("保留を解除") }
                        }
                        Button(
                            onClick = onSend, enabled = canSend,
                            modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp),
                        ) {
                            if (state.busy) {
                                CircularProgressIndicator(Modifier.size(16.dp), color = colors.onSurfaceVariant, strokeWidth = 2.dp)
                                Spacer(Modifier.width(8.dp))
                            }
                            Text(if (state.busy) "保存中" else if (pending) "再試行" else "保存")
                        }
                    }
                }
            }
        },
    ) { insets ->
        Box(Modifier.fillMaxSize().padding(insets), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = 560.dp).fillMaxWidth().verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp, vertical = 16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                if (link != null && !editBody) {
                    Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.large, color = colors.surfaceContainerHigh) {
                        Row(
                            Modifier.padding(start = 15.dp, top = 9.dp, bottom = 9.dp, end = 5.dp),
                            horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically,
                        ) {
                            MoteIcon(MoteGlyph.Link, null, tint = colors.primary)
                            Column(Modifier.weight(1f)) {
                                Text(link.host, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                if (link.url != "https://${link.host}" && link.url != "https://${link.host}/") {
                                    Text(link.url, color = colors.onSurfaceVariant, style = MaterialTheme.typography.bodySmall,
                                        maxLines = 1, overflow = TextOverflow.Ellipsis)
                                }
                            }
                            IconButton(onClick = { editBody = true }, enabled = state.sharedAvailable && !readOnly) {
                                MoteIcon(MoteGlyph.Edit, "リンクを編集", Modifier.size(19.dp))
                            }
                        }
                    }
                    MoteComposer(
                        value = link.note, onValueChange = { onDraft(link.withNote(it)) },
                        placeholder = "メモを追加…", enabled = state.sharedAvailable, readOnly = readOnly, maxLines = 6,
                        actionCount = 0,
                    ) {}
                } else {
                    MoteComposer(
                        value = state.sharedDraft, onValueChange = onDraft, placeholder = "capture を書く…",
                        enabled = state.sharedAvailable, readOnly = readOnly, maxLines = 8,
                        actionCount = 0,
                    ) {}
                }
                if (needsReview || state.sharedMessage != null) SharedCaptureNotice(needsReview, state.sharedMessage)
                SharedCaptureConnection(state.connection, state.busy, onConnect, onAuthorize)
            }
        }
    }
}

internal data class SharedLinkDraft(val url: String, val host: String, val note: String) {
    fun withNote(text: String): String = if (text.isEmpty()) url else "$url\n\n$text"
}

/** Project a standalone HTTP(S) share without rewriting the URL or separating stored data. */
internal fun sharedLinkDraft(body: String): SharedLinkDraft? {
    val separator = body.indexOf("\n\n")
    val url = if (separator < 0) body else body.substring(0, separator)
    val link = try {
        URI(url)
    } catch (_: java.net.URISyntaxException) {
        return null
    }
    val host = link.host ?: return null
    if (!link.scheme.equals("http", ignoreCase = true) && !link.scheme.equals("https", ignoreCase = true)) return null
    val note = if (separator < 0) "" else body.substring(separator + 2)
    return SharedLinkDraft(url, host, note)
}

@Composable
private fun SharedCaptureNotice(pending: Boolean, message: String?) {
    val colors = MaterialTheme.colorScheme
    Surface(
        modifier = Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite },
        shape = MaterialTheme.shapes.medium,
        color = colors.tertiaryContainer,
        contentColor = colors.onTertiaryContainer,
    ) {
        Row(Modifier.padding(12.dp), horizontalArrangement = Arrangement.spacedBy(9.dp)) {
            MoteIcon(MoteGlyph.Alert, null, Modifier.size(18.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                if (pending) {
                    Text("送信結果を確認できていません", style = MaterialTheme.typography.labelLarge)
                    Text(
                        "すでに保存されている可能性があります。「閉じる」を選んでも、再試行できるよう下書きは残ります。",
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                if (message != null) Text(message, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

@Composable
private fun SharedCaptureConnection(
    connection: MoteConnection,
    busy: Boolean,
    onConnect: () -> Unit,
    onAuthorize: () -> Unit,
) {
    val label = when (connection) {
        MoteConnection.Loading -> "Harmion に接続中"
        MoteConnection.Approving -> "Harmion でアクセスを許可してください"
        MoteConnection.NeedsApproval -> "保存するには Harmion の承認が必要です"
        MoteConnection.Unavailable, MoteConnection.Failed -> "Harmion に未接続です"
        MoteConnection.Ready -> return
    }
    val colors = MaterialTheme.colorScheme
    Surface(
        modifier = Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite },
        shape = MaterialTheme.shapes.medium,
        color = colors.surfaceContainer,
        contentColor = colors.onSurfaceVariant,
    ) {
        Column(Modifier.padding(start = 12.dp, end = 12.dp, top = 9.dp, bottom = 9.dp)) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(9.dp),
            ) {
                if (connection == MoteConnection.Loading || connection == MoteConnection.Approving) {
                    CircularProgressIndicator(Modifier.size(16.dp), color = colors.secondary, strokeWidth = 2.dp)
                } else {
                    MoteIcon(MoteGlyph.Link, null, Modifier.size(16.dp), tint = colors.secondary)
                }
                Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall)
            }
            when (connection) {
                MoteConnection.NeedsApproval -> TextButton(
                    onClick = onAuthorize,
                    modifier = Modifier.align(Alignment.End),
                    enabled = !busy,
                ) { Text("承認する") }
                MoteConnection.Unavailable, MoteConnection.Failed -> TextButton(
                    onClick = onConnect,
                    modifier = Modifier.align(Alignment.End),
                    enabled = !busy,
                ) { Text("再接続") }
                else -> Unit
            }
        }
    }
}
