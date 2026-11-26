package org.harmion.mote

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.SystemClock
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalIconToggleButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Snackbar
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxState
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.ZonedDateTime
import java.time.format.DateTimeFormatter
import kotlinx.coroutines.delay

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun MoteScreen(
    state: MoteUiState,
    onDraft: (String) -> Unit,
    onSubmit: () -> Unit,
    onConnect: () -> Unit,
    onAuthorize: () -> Unit,
    onArchive: (Capture, Boolean) -> Unit,
    onPin: (Capture, Boolean) -> Unit,
    onDelete: (Capture) -> Unit,
    archiveNotice: ArchiveNotice?,
    onUndoArchive: (ArchiveNotice) -> Unit,
    onDismissArchive: (ArchiveNotice) -> Unit,
    onReleasePending: () -> Unit,
    onOpenLong: () -> Unit,
    onLeaveLong: () -> Unit,
    onLongDraft: (String) -> Unit,
    onAppendLong: () -> Unit,
    onFinishLong: () -> Unit,
    onCheckLong: () -> Unit,
    onForgetLong: () -> Unit,
    preferences: MotePreferences,
    onOpenSettings: () -> Unit,
) {
    val ready = state.connection == MoteConnection.Ready
    val canMutate = ready && !state.busy
    val context = LocalContext.current
    val colors = MaterialTheme.colorScheme
    var includeArchived by rememberSaveable { mutableStateOf(false) }
    var pinnedOnly by rememberSaveable { mutableStateOf(false) }
    var selectedId by rememberSaveable { mutableStateOf<String?>(null) }
    var deleteSnapshot by remember { mutableStateOf<Capture?>(null) }
    var confirmRelease by remember { mutableStateOf(false) }
    var completedVisible by remember { mutableStateOf(false) }
    val allCaptures = if (ready) state.captures else emptyList()
    val selected = selectedId?.let { id -> allCaptures.firstOrNull { it.id == id } }
    val deleteTarget = deleteSnapshot?.takeIf { target -> allCaptures.any { it.id == target.id && !it.sessionOpen } }

    SideEffect {
        if (selected == null) selectedId = null
        if (deleteTarget == null) deleteSnapshot = null
        if (!state.pending) confirmRelease = false
    }

    val notice = archiveNotice?.takeIf { ready }
    val snackbar = remember { SnackbarHostState() }
    val undo by rememberUpdatedState(onUndoArchive)
    val dismiss by rememberUpdatedState(onDismissArchive)
    LaunchedEffect(notice?.sequence) {
        if (notice != null) {
            val result = snackbar.showSnackbar(
                message = if (notice.archived) "アーカイブしました" else "Inbox に戻しました",
                actionLabel = "元に戻す",
                withDismissAction = true,
                duration = SnackbarDuration.Long,
            )
            if (result == SnackbarResult.ActionPerformed) undo(notice) else dismiss(notice)
        }
    }
    LaunchedEffect(state.longCompletion) {
        val remaining = 1900L - (SystemClock.elapsedRealtime() - state.longCompletion)
        completedVisible = state.longCompletion != 0L && remaining > 0
        if (completedVisible) {
            delay(remaining)
            completedVisible = false
        }
    }

    val long = state.longCapture
    if (state.longCaptureVisible && long != null) {
        LongCaptureScreen(
            state = long, onLeave = onLeaveLong, onDraft = onLongDraft, onAppend = onAppendLong,
            onFinish = onFinishLong, onCheck = onCheckLong, onForget = onForgetLong,
            connection = state.connection, onConnect = onConnect, onAuthorize = onAuthorize,
        )
        return
    }

    Scaffold(
        modifier = Modifier.imePadding(),
        containerColor = colors.background,
        topBar = {
            MoteTopBar(
                capture = selected,
                onBack = { selectedId = null },
                onOpenSettings = onOpenSettings,
                onCopy = { selected?.let { copyCapture(context, it.body) } },
                onShare = { selected?.let { shareCapture(context, it.body) } },
                onDelete = { deleteSnapshot = selected },
                canDelete = canMutate && selected?.sessionOpen == false && selected.id !in state.archivePreviews,
            )
        },
        bottomBar = {
            if (selected == null) {
                CaptureDock(state, includeArchived, pinnedOnly, { includeArchived = it }, { pinnedOnly = it },
                    onDraft, onSubmit, { confirmRelease = true }, onOpenLong, completedVisible && notice == null,
                    preferences.completionMotion)
            } else {
                DetailActions(
                    selected, canMutate && selected.id !in state.archivePreviews,
                    onArchive = { onArchive(selected, !selected.archived) },
                    onPin = { onPin(selected, !selected.pinned) },
                )
            }
        },
        snackbarHost = {
            SnackbarHost(snackbar) { data ->
                Snackbar(
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 5.dp),
                    shape = MaterialTheme.shapes.medium,
                    containerColor = colors.surfaceContainerHigh,
                    contentColor = colors.onSurface,
                    action = { TextButton(onClick = data::performAction, enabled = canMutate) { Text("元に戻す") } },
                    dismissAction = {
                        IconButton(onClick = data::dismiss) { MoteIcon(MoteGlyph.Close, "閉じる", Modifier.size(18.dp)) }
                    },
                ) { Text(data.visuals.message) }
            }
        },
    ) { insets ->
        Box(Modifier.fillMaxSize().padding(insets).consumeWindowInsets(insets)) {
            Column(Modifier.fillMaxSize()) {
                if (state.longCaptureStorageFailed) InlineNotice("端末の長文記録を読み込めません。長文入力は停止しています。")
                state.message?.takeUnless { it in quietSuccessMessages }?.let { InlineNotice(it) }
                when {
                    !ready -> ConnectionStatus(state.connection, onConnect, onAuthorize, Modifier.weight(1f))
                    selected != null -> {
                        BackHandler(enabled = deleteTarget == null) { selectedId = null }
                        key(selected.id) { CaptureDetail(selected, Modifier.weight(1f)) }
                    }
                    else -> {
                        val localLongId = state.longCapture?.captureId
                        val filtered = remember(allCaptures, includeArchived, pinnedOnly, state.archivePreviews, localLongId) {
                            val projected = allCaptures.mapNotNull { item ->
                                if (item.sessionOpen && item.id == localLongId) null
                                else state.archivePreviews[item.id]?.let { item.copy(archived = it.archived) } ?: item
                            }
                            CaptureFilter(includeArchived, pinnedOnly).applyTo(projected)
                        }
                        if (filtered.isEmpty()) {
                            BoxWithSafeHorizontal(Modifier.weight(1f)) {
                                Text(
                                    if (pinnedOnly) "ピン留めはありません" else if (includeArchived) "メモはありません" else "Inbox は空です",
                                    modifier = Modifier.align(Alignment.Center).padding(horizontal = 20.dp),
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = colors.onSurfaceVariant,
                                    textAlign = TextAlign.Center,
                                )
                            }
                        } else {
                            key(includeArchived, pinnedOnly) {
                                CaptureTimeline(
                                    filtered, Modifier.weight(1f),
                                    enabled = canMutate, updating = state.archivePreviews.keys,
                                    archiveHaptics = preferences.archiveHaptics,
                                    onSelect = { selectedId = it.id },
                                    onArchive = { onArchive(it, !it.archived) },
                                    onPin = { onPin(it, !it.pinned) },
                                    onCopy = { copyCapture(context, it.body) },
                                    onShare = { shareCapture(context, it.body) },
                                    onDelete = { deleteSnapshot = it },
                                )
                            }
                        }
                    }
                }
            }
        }
    }

    if (deleteTarget != null) {
        DeleteCaptureDialog(deleteTarget, canMutate && deleteTarget.id !in state.archivePreviews, { deleteSnapshot = null }) {
            deleteSnapshot = null
            onDelete(deleteTarget)
        }
    }
    if (confirmRelease && state.pending && state.draftAvailable) {
        AlertDialog(
            onDismissRequest = { confirmRelease = false },
            title = { Text("下書きの編集を再開？") },
            text = { Text("前回の送信は保存済みの可能性があります。編集して送信すると、別のメモとして保存されます。") },
            confirmButton = {
                TextButton(enabled = !state.busy, onClick = { confirmRelease = false; onReleasePending() }) { Text("編集を再開") }
            },
            dismissButton = { TextButton(onClick = { confirmRelease = false }) { Text("キャンセル") } },
        )
    }
}

private val quietSuccessMessages = setOf("Harmion に保存しました。", "削除しました。", "長文メモを閉じました。")

@Composable
private fun MoteTopBar(
    capture: Capture?, onBack: () -> Unit, onOpenSettings: () -> Unit,
    onCopy: () -> Unit, onShare: () -> Unit, onDelete: () -> Unit, canDelete: Boolean,
) {
    var menu by remember(capture?.id) { mutableStateOf(false) }
    val colors = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal))) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 62.dp).padding(start = if (capture == null) 20.dp else 6.dp, end = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (capture == null) {
                Box(
                    Modifier.size(38.dp).background(colors.primaryContainer, RoundedCornerShape(13.dp)),
                    contentAlignment = Alignment.Center,
                ) { MoteMark(Modifier.size(24.dp)) }
                Spacer(Modifier.width(12.dp))
            } else {
                IconButton(onClick = onBack) { MoteIcon(MoteGlyph.Back, "戻る") }
                Spacer(Modifier.width(5.dp))
            }
            Text(
                when {
                    capture == null -> "Mote"
                    capture.isLong -> "長文"
                    else -> "メモ"
                },
                Modifier.weight(1f).semantics { heading() },
                style = if (capture == null) MaterialTheme.typography.headlineLarge else MaterialTheme.typography.titleLarge,
            )
            if (capture == null) {
                IconButton(onClick = onOpenSettings) { MoteIcon(MoteGlyph.Settings, "設定", tint = colors.onSurfaceVariant) }
            } else {
                Box {
                    IconButton(onClick = { menu = true }) { MoteIcon(MoteGlyph.More, "その他の操作", tint = colors.onSurfaceVariant) }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        MenuAction(MoteGlyph.Copy, "コピー", enabled = capture.body.isNotEmpty()) { menu = false; onCopy() }
                        MenuAction(MoteGlyph.Share, "共有", enabled = capture.body.isNotEmpty()) { menu = false; onShare() }
                        MenuAction(MoteGlyph.Delete, "削除…", enabled = canDelete, destructive = true) { menu = false; onDelete() }
                    }
                }
            }
        }
        if (capture != null) HorizontalDivider(color = colors.outlineVariant, modifier = Modifier.padding(horizontal = 20.dp))
    }
}

@Composable
private fun InlineNotice(text: String) {
    val colors = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 8.dp).semantics { liveRegion = LiveRegionMode.Polite },
        horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically,
    ) {
        MoteIcon(MoteGlyph.Alert, null, Modifier.size(16.dp), colors.secondary)
        Text(text, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
    }
}

@Composable
private fun ConnectionStatus(connection: MoteConnection, onConnect: () -> Unit, onAuthorize: () -> Unit, modifier: Modifier) {
    when (connection) {
        MoteConnection.Loading -> CenteredState("Harmion に接続中", "この端末で下書きを続けられます。", modifier, loading = true)
        MoteConnection.Approving -> CenteredState("承認を待っています", "Harmion でアクセスを許可してください。", modifier, loading = true)
        MoteConnection.NeedsApproval -> CenteredState("Harmion とつなぐ", "Mote のメモだけにアクセスします。", modifier, mark = true) {
            Button(onClick = onAuthorize) { Text("承認をリクエスト") }
        }
        MoteConnection.Unavailable, MoteConnection.Failed -> CenteredState("Harmion に未接続", "入力した下書きはこの端末に残ります。", modifier, glyph = MoteGlyph.Link) {
            Button(onClick = onConnect) { Text("再接続") }
        }
        MoteConnection.Ready -> Unit
    }
}

@Composable
private fun CenteredState(
    title: String, note: String, modifier: Modifier, loading: Boolean = false, mark: Boolean = false,
    glyph: MoteGlyph = MoteGlyph.Inbox, action: @Composable () -> Unit = {},
) {
    BoxWithSafeHorizontal(modifier) {
        Column(
            Modifier.align(Alignment.Center).padding(horizontal = 24.dp).verticalScroll(rememberScrollState()),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(7.dp),
        ) {
            when {
                loading -> CircularProgressIndicator(Modifier.size(25.dp), strokeWidth = 2.dp)
                mark -> MoteMark(Modifier.size(44.dp), MaterialTheme.colorScheme.primary)
                else -> MoteIcon(glyph, null, Modifier.size(30.dp), MaterialTheme.colorScheme.secondary)
            }
            Spacer(Modifier.height(4.dp))
            Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
            Text(note, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
            action()
        }
    }
}

@Composable
private fun BoxWithSafeHorizontal(modifier: Modifier, content: @Composable androidx.compose.foundation.layout.BoxScope.() -> Unit) {
    Box(modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal)), content = content)
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun CaptureDock(
    state: MoteUiState, includeArchived: Boolean, pinnedOnly: Boolean,
    onIncludeArchived: (Boolean) -> Unit, onPinnedOnly: (Boolean) -> Unit,
    onDraft: (String) -> Unit, onSubmit: () -> Unit, onRelease: () -> Unit, onOpenLong: () -> Unit,
    completedVisible: Boolean, animateCompletion: Boolean,
) {
    val colors = MaterialTheme.colorScheme
    val ready = state.connection == MoteConnection.Ready
    val canSubmit = ready && state.draftAvailable && !state.busy && state.draft.isNotBlank()
    val canOpenLong = state.longCaptureAvailable && !state.busy
    MoteDock {
        val long = state.longCapture
        if (long != null) {
            Surface(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(18.dp),
                color = colors.surfaceContainerHigh,
            ) {
                Row(
                    Modifier.padding(start = 13.dp, end = 5.dp, top = 3.dp, bottom = 3.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text("長文の下書き", style = MaterialTheme.typography.labelLarge)
                        val preview = long.draft.ifBlank { long.body }.trimStart()
                        if (preview.isNotEmpty()) {
                            Text(
                                preview, color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall,
                                maxLines = 1, overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                    TextButton(onClick = onOpenLong, enabled = canOpenLong) { Text("再開") }
                }
            }
        }
        AnimatedVisibility(
            visible = completedVisible,
            enter = if (animateCompletion) fadeIn(MaterialTheme.motionScheme.fastEffectsSpec()) else EnterTransition.None,
            exit = if (animateCompletion) fadeOut(MaterialTheme.motionScheme.fastEffectsSpec()) else ExitTransition.None,
        ) { LongCompletionNotice(state.longCompletion, animateCompletion) }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
            Spacer(Modifier.weight(1f))
            SingleChoiceSegmentedButtonRow {
                listOf("Inbox", "All").forEachIndexed { index, label ->
                    SegmentedButton(
                        selected = includeArchived == (index == 1), onClick = { onIncludeArchived(index == 1) },
                        enabled = ready, shape = SegmentedButtonDefaults.itemShape(index, 2),
                        colors = SegmentedButtonDefaults.colors(
                            activeContainerColor = colors.primaryContainer, activeContentColor = colors.onPrimaryContainer,
                            activeBorderColor = colors.outlineVariant, inactiveContainerColor = colors.surfaceContainerLow,
                            inactiveContentColor = colors.onSurfaceVariant, inactiveBorderColor = colors.outlineVariant,
                        ),
                        icon = {},
                    ) { Text(label, maxLines = 1) }
                }
            }
            FilledTonalIconToggleButton(
                checked = pinnedOnly, onCheckedChange = onPinnedOnly, enabled = ready,
                shapes = IconButtonDefaults.toggleableShapes(),
                colors = IconButtonDefaults.filledTonalIconToggleButtonColors(
                    containerColor = colors.surfaceContainer, contentColor = colors.onSurfaceVariant,
                    checkedContainerColor = colors.primaryContainer, checkedContentColor = colors.primary,
                ),
            ) { MoteIcon(MoteGlyph.Pin, "ピン留めのみ", Modifier.size(19.dp)) }
        }
        if (state.pending && !state.busy) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text("送信結果を確認できません", Modifier.weight(1f), style = MaterialTheme.typography.labelMedium, color = colors.tertiary)
                TextButton(onClick = onRelease, enabled = !state.busy) { Text("下書きを編集") }
            }
        }
        MoteComposer(state.draft, onDraft, "メモを書く…", enabled = state.draftAvailable, readOnly = state.pending || state.busy) {
            if (state.longCapture == null) {
                IconButton(onClick = onOpenLong, enabled = canOpenLong) {
                    MoteIcon(MoteGlyph.Expand, "長文を書く", Modifier.size(20.dp), colors.onSurfaceVariant.copy(alpha = if (canOpenLong) 1f else .45f))
                }
            }
            MoteSendButton(canSubmit, if (state.pending) "再送" else "送信", onSubmit)
        }
    }
}

@Composable
private fun MoteDock(content: @Composable () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Surface(color = colors.background) {
        Box(Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Bottom)),
            contentAlignment = Alignment.Center) {
            Column(Modifier.widthIn(max = 680.dp).fillMaxWidth().padding(horizontal = 20.dp, vertical = 7.dp),
                verticalArrangement = Arrangement.spacedBy(5.dp)) {
                content()
            }
        }
    }
}

@Composable
private fun LongCompletionNotice(sequence: Long, animate: Boolean) {
    val colors = MaterialTheme.colorScheme
    val progress = remember { Animatable(1f) }
    LaunchedEffect(sequence, animate) {
        if (animate) {
            progress.snapTo(0f)
            progress.animateTo(1f, tween(durationMillis = 360, easing = LinearOutSlowInEasing))
        }
    }
    Box(Modifier.fillMaxWidth().drawWithContent {
        drawContent()
        val frame = progress.value
        val alpha = (1f - frame) * (frame * 5f).coerceAtMost(1f)
        if (animate && alpha > 0f) {
            for (index in 0..2) {
                drawCircle(
                    color = colors.primary, alpha = alpha,
                    radius = (1.5f + index / 3f).dp.toPx(),
                    center = Offset(size.width - (22 + index * 10).dp.toPx() - 14.dp.toPx() * frame,
                        15.dp.toPx() - (12 + index * 4).dp.toPx() * frame),
                )
            }
        }
    }.padding(top = 9.dp)) {
        Surface(Modifier.fillMaxWidth(), shape = CircleShape, color = colors.primaryContainer, contentColor = colors.onPrimaryContainer) {
            Row(Modifier.padding(horizontal = 14.dp, vertical = 9.dp).semantics { liveRegion = LiveRegionMode.Polite },
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                MoteIcon(MoteGlyph.Check, null, Modifier.size(17.dp), colors.primary)
                Text("長文を保存しました", style = MaterialTheme.typography.labelMedium)
            }
        }
    }
}

@Composable
private fun CaptureTimeline(
    captures: List<Capture>, modifier: Modifier, enabled: Boolean, updating: Set<String>, archiveHaptics: Boolean,
    onSelect: (Capture) -> Unit, onArchive: (Capture) -> Unit, onPin: (Capture) -> Unit,
    onCopy: (Capture) -> Unit, onShare: (Capture) -> Unit, onDelete: (Capture) -> Unit,
) {
    val zone = ZoneId.systemDefault()
    val today = LocalDate.now(zone)
    val days = remember(captures, zone) {
        captures.asReversed().groupBy { it.createdAtMicros?.atZone(zone)?.toLocalDate() }
    }
    val listState = rememberLazyListState()
    val latest = captures.last().id
    val followLatest = listState.firstVisibleItemIndex == 0 &&
        listState.firstVisibleItemScrollOffset == 0 && !listState.isScrollInProgress
    LaunchedEffect(latest) {
        if (followLatest) listState.scrollToItem(0)
    }
    val horizontal = WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal).asPaddingValues()
    Box(modifier.fillMaxWidth().padding(horizontal), contentAlignment = Alignment.Center) {
        LazyColumn(
            Modifier.widthIn(max = 680.dp).fillMaxSize(),
            state = listState,
            reverseLayout = true,
            contentPadding = PaddingValues(horizontal = 20.dp, vertical = 5.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp, Alignment.Bottom),
        ) {
            days.forEach { (day, entries) ->
                items(entries, key = Capture::id, contentType = { "capture" }) { capture ->
                    CaptureRow(capture, enabled && capture.id !in updating, archiveHaptics, onSelect, onArchive, onPin, onCopy, onShare, onDelete)
                }
                item(key = "day:$day", contentType = "day") { TimelineDay(day, today) }
            }
        }
    }
}

@Composable
private fun TimelineDay(day: LocalDate?, today: LocalDate) {
    val label = when (day) {
        null -> "日時不明"
        today -> "今日"
        today.minusDays(1) -> "昨日"
        else -> if (day.year == today.year) "${day.monthValue}月${day.dayOfMonth}日" else "${day.year}年${day.monthValue}月${day.dayOfMonth}日"
    }
    val colors = MaterialTheme.colorScheme
    Box(Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 3.dp), contentAlignment = Alignment.Center) {
        Text(label, Modifier.semantics { heading() }, color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun CaptureRow(
    capture: Capture, enabled: Boolean, archiveHaptics: Boolean, onSelect: (Capture) -> Unit, onArchive: (Capture) -> Unit,
    onPin: (Capture) -> Unit, onCopy: (Capture) -> Unit, onShare: (Capture) -> Unit, onDelete: (Capture) -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    val colors = MaterialTheme.colorScheme
    val haptic = LocalHapticFeedback.current
    val leftSwipe = if (LocalLayoutDirection.current == LayoutDirection.Ltr) SwipeToDismissBoxValue.EndToStart else SwipeToDismissBoxValue.StartToEnd
    val swipe = remember(capture.archived, leftSwipe) {
        SwipeToDismissBoxState(SwipeToDismissBoxValue.Settled, positionalThreshold = { width -> (width * .35f) })
    }
    LaunchedEffect(swipe.settledValue) {
        if (swipe.settledValue == leftSwipe) {
            if (enabled && !menu) {
                if (archiveHaptics) haptic.performHapticFeedback(HapticFeedbackType.Confirm)
                onArchive(capture)
            }
            swipe.reset()
        }
    }
    val armed = swipe.targetValue == leftSwipe
    val swipeColor by animateColorAsState(if (armed) colors.secondary else colors.secondaryContainer,
        MaterialTheme.motionScheme.fastEffectsSpec(), label = "archive threshold")
    val shape = if (capture.isLong) RoundedCornerShape(18.dp) else
        RoundedCornerShape(topStart = 17.dp, topEnd = 17.dp, bottomEnd = 7.dp, bottomStart = 17.dp)
    val preview = remember(capture.body) {
        val body = capture.body.trimStart()
        // The row has at most three lines. Keep the rest of a long body out of text layout.
        if (body.length <= 640) body.ifBlank { "（本文なし）" }
        else body.take(if (body[640].isLowSurrogate()) 639 else 640) + "…"
    }
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        val fraction = when {
            capture.isLong -> 1f
            preview.length <= 13 -> .64f
            preview.length <= 25 -> .79f
            else -> .93f
        }
        SwipeToDismissBox(
            state = swipe, modifier = Modifier.fillMaxWidth(fraction).clip(shape),
            enableDismissFromStartToEnd = leftSwipe == SwipeToDismissBoxValue.StartToEnd,
            enableDismissFromEndToStart = leftSwipe == SwipeToDismissBoxValue.EndToStart,
            gesturesEnabled = enabled && !menu,
            backgroundContent = {
                Box(Modifier.fillMaxSize().background(swipeColor).padding(horizontal = 14.dp).clearAndSetSemantics {},
                    contentAlignment = if (leftSwipe == SwipeToDismissBoxValue.EndToStart) Alignment.CenterEnd else Alignment.CenterStart) {
                    val tint = if (armed) colors.onSecondary else colors.onSecondaryContainer
                    Row(horizontalArrangement = Arrangement.spacedBy(7.dp), verticalAlignment = Alignment.CenterVertically) {
                        MoteIcon(if (capture.archived) MoteGlyph.Restore else MoteGlyph.Archive, null, Modifier.size(18.dp), tint)
                        Text(archiveLabel(capture), style = MaterialTheme.typography.labelMedium, color = tint)
                    }
                }
            },
        ) {
            Box(Modifier.fillMaxWidth()) {
                val outline = if (capture.isLong) Modifier.border(1.dp, colors.outlineVariant, shape) else Modifier
                Column(
                    Modifier.fillMaxWidth().background(if (capture.isLong) colors.surfaceContainer else colors.surfaceContainerHigh)
                        .then(outline)
                        .combinedClickable(
                            onClickLabel = if (capture.isLong) "長文を開く" else "メモを開く",
                            onClick = { onSelect(capture) },
                            onLongClickLabel = "メモの操作",
                            onLongClick = { menu = true },
                        ).padding(horizontal = 13.dp, vertical = if (capture.isLong) 10.dp else 7.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    if (capture.isLong) {
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            Text(if (capture.sessionOpen) "長文・下書き" else "長文",
                                Modifier.weight(1f), color = colors.primary, style = MaterialTheme.typography.labelSmall)
                            CaptureRowMetadata(capture)
                        }
                    }
                    Text(preview, style = MaterialTheme.typography.bodyMedium,
                        maxLines = 3, overflow = TextOverflow.Ellipsis)
                    if (!capture.isLong) {
                        Box(Modifier.align(Alignment.End)) { CaptureRowMetadata(capture) }
                    }
                }
                CaptureMenu(menu, { menu = false }, capture, enabled, onArchive, onPin, onCopy, onShare, onDelete)
            }
        }
    }
}

@Composable
private fun CaptureRowMetadata(capture: Capture) {
    val colors = MaterialTheme.colorScheme
    Row(horizontalArrangement = Arrangement.spacedBy(5.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(capture.createdAtMicros?.atZone(ZoneId.systemDefault())?.format(shortTime) ?: "日時不明",
            color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
        if (capture.archived) MoteIcon(MoteGlyph.Archive, "アーカイブ済み", Modifier.size(13.dp), colors.onSurfaceVariant)
        if (capture.pinned) MoteIcon(MoteGlyph.Pin, "ピン留め済み", Modifier.size(13.dp), colors.primary)
    }
}

@Composable
private fun CaptureMenu(
    visible: Boolean, close: () -> Unit, capture: Capture, enabled: Boolean,
    onArchive: (Capture) -> Unit, onPin: (Capture) -> Unit, onCopy: (Capture) -> Unit,
    onShare: (Capture) -> Unit, onDelete: (Capture) -> Unit,
) {
    DropdownMenu(visible, close) {
        MenuAction(MoteGlyph.Pin, pinLabel(capture), enabled) { close(); onPin(capture) }
        MenuAction(MoteGlyph.Copy, "コピー", capture.body.isNotEmpty()) { close(); onCopy(capture) }
        MenuAction(MoteGlyph.Share, "共有", capture.body.isNotEmpty()) { close(); onShare(capture) }
        MenuAction(if (capture.archived) MoteGlyph.Restore else MoteGlyph.Archive, archiveLabel(capture), enabled) { close(); onArchive(capture) }
        HorizontalDivider(Modifier.padding(vertical = 4.dp), color = MaterialTheme.colorScheme.outlineVariant)
        MenuAction(MoteGlyph.Delete, "削除…", enabled && !capture.sessionOpen, destructive = true) { close(); onDelete(capture) }
    }
}

@Composable
private fun MenuAction(glyph: MoteGlyph, label: String, enabled: Boolean = true, destructive: Boolean = false, action: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val tint = if (destructive && enabled) colors.error else colors.onSurfaceVariant.copy(alpha = if (enabled) 1f else .38f)
    DropdownMenuItem(
        text = { if (destructive && enabled) Text(label, color = colors.error) else Text(label) },
        onClick = action, enabled = enabled,
        leadingIcon = { MoteIcon(glyph, null, Modifier.size(19.dp), tint) },
    )
}

@Composable
private fun CaptureMetadata(capture: Capture) {
    val date = capture.createdAtMicros?.atZone(ZoneId.systemDefault())
    val time = date?.format(detailTime) ?: "日時不明"
    val labels = buildList {
        add(time)
        if (capture.archived) add("アーカイブ済み")
        if (capture.sessionOpen) add("下書き")
        if (capture.pinned) add("ピン留め済み")
    }
    Text(labels.joinToString("  ·  "), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

@Composable
private fun CaptureDetail(capture: Capture, modifier: Modifier) {
    BoxWithSafeHorizontal(modifier) {
        val body = capture.body.ifBlank { "（本文なし）" }
        val slices = remember(body, capture.isLong) { if (capture.isLong) threadSlices(body) else emptyList() }
        SelectionContainer(Modifier.align(Alignment.TopCenter).widthIn(max = 680.dp).fillMaxSize()) {
            LazyColumn(contentPadding = PaddingValues(horizontal = 20.dp, vertical = 18.dp)) {
                item(key = "metadata") {
                    Column(Modifier.padding(bottom = 14.dp)) { CaptureMetadata(capture) }
                }
                if (capture.isLong) {
                    items(slices, key = { it.start }) { slice -> LongCaptureThreadSlice(body, slice) }
                } else {
                    item { Text(body, style = MaterialTheme.typography.bodyLarge) }
                }
            }
        }
    }
}

@Composable
private fun DetailActions(capture: Capture, enabled: Boolean, onArchive: () -> Unit, onPin: () -> Unit) {
    MoteDock {
        Row(Modifier.fillMaxWidth().padding(top = 3.dp), horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.End), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = onPin, enabled = enabled) {
                MoteIcon(MoteGlyph.Pin, null, Modifier.size(17.dp)); Spacer(Modifier.width(7.dp)); Text(pinLabel(capture))
            }
            TextButton(onClick = onArchive, enabled = enabled) {
                MoteIcon(if (capture.archived) MoteGlyph.Restore else MoteGlyph.Archive, null, Modifier.size(17.dp))
                Spacer(Modifier.width(7.dp)); Text(archiveLabel(capture))
            }
        }
    }
}

@Composable
private fun DeleteCaptureDialog(capture: Capture, enabled: Boolean, onCancel: () -> Unit, onConfirm: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    AlertDialog(
        onDismissRequest = onCancel,
        icon = { MoteIcon(MoteGlyph.Delete, null, tint = colors.error) },
        title = { Text("このメモを削除？") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(9.dp)) {
                Text("以下の 1 件を Harmion から削除します。アーカイブとは異なり、取り消せません。")
                HorizontalDivider(color = colors.outlineVariant)
                CaptureMetadata(capture)
                Text(capture.body.ifBlank { "（本文なし）" }, maxLines = 5, overflow = TextOverflow.Ellipsis, color = colors.onSurface)
            }
        },
        confirmButton = {
            Button(onClick = onConfirm, enabled = enabled,
                colors = ButtonDefaults.buttonColors(containerColor = colors.errorContainer, contentColor = colors.onErrorContainer)) { Text("削除") }
        },
        dismissButton = { TextButton(onClick = onCancel) { Text("キャンセル") } },
    )
}

private fun archiveLabel(capture: Capture) = if (capture.archived) "Inbox に戻す" else "アーカイブ"
private fun pinLabel(capture: Capture) = if (capture.pinned) "ピン留め解除" else "ピン留め"
private val shortTime = DateTimeFormatter.ofPattern("HH:mm")
private val detailTime = DateTimeFormatter.ofPattern("yyyy/MM/dd  HH:mm")

private fun Long.atZone(zone: ZoneId): ZonedDateTime =
    Instant.ofEpochSecond(Math.floorDiv(this, 1_000_000L), Math.floorMod(this, 1_000_000L) * 1_000L).atZone(zone)

private fun copyCapture(context: Context, text: String) {
    context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(ClipData.newPlainText("Mote", text))
    if (android.os.Build.VERSION.SDK_INT < 33) android.widget.Toast.makeText(context, "コピーしました", android.widget.Toast.LENGTH_SHORT).show()
}

private fun shareCapture(context: Context, text: String) {
    val payload = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
    context.startActivity(Intent.createChooser(payload, null))
}
