package org.harmion.mote

import android.annotation.SuppressLint
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.SystemClock
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.togetherWith
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.ui.graphics.Brush
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.animateScrollBy
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
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
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalIconToggleButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Snackbar
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxState
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import kotlinx.coroutines.launch
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.toMutableStateList
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.ZonedDateTime
import java.time.format.DateTimeFormatter
import kotlinx.coroutines.delay

private const val CollapsedShortLines = 7
private const val MaxTextWidthFraction = .97f

private fun gapPadding(micros: Long?): Dp = when {
    micros == null || micros < 15L * 60 * 1_000_000 -> 0.dp
    micros < 2L * 60 * 60 * 1_000_000 -> 8.dp
    else -> 18.dp
}

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
    onDeleteLongDraft: () -> Unit,
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
    onReply: (Capture) -> Unit,
    onCancelReply: () -> Unit,
    onPushToLong: (List<String>) -> Unit,
    onBeginEdit: (Capture) -> Unit,
    onCancelEdit: () -> Unit,
    onSync: () -> Unit,
    preferences: MotePreferences,
    onOpenSettings: () -> Unit,
) {
    val ready = state.connection == MoteConnection.Ready
    val canMutate = ready && !state.busy
    val context = LocalContext.current
    val colors = MaterialTheme.colorScheme
    val density = LocalDensity.current
    val focusManager = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    val haptic = LocalHapticFeedback.current
    val backdrop = rememberMoteBackdrop()
    val homeList = rememberLazyListState()
    var expandAnchor by remember { mutableStateOf<ExpandAnchor?>(null) }
    val replyFocus = remember { androidx.compose.ui.focus.FocusRequester() }
    var requestReplyFocus by remember { mutableStateOf(false) }
    var archivedOnly by rememberSaveable { mutableStateOf(false) }
    var pinnedOnly by rememberSaveable { mutableStateOf(false) }
    var selectedId by rememberSaveable { mutableStateOf<String?>(null) }
    var deleteSnapshot by remember { mutableStateOf<Capture?>(null) }
    var confirmRelease by remember { mutableStateOf(false) }
    var confirmDeleteDraft by remember { mutableStateOf(false) }
    var confirmPush by remember { mutableStateOf(false) }
    var selecting by rememberSaveable { mutableStateOf(false) }
    val selectedIds = rememberSaveable(
        saver = listSaver(save = { it.toList() }, restore = { it.toMutableStateList() }),
    ) { mutableStateListOf<String>() }
    val expandedIds = rememberSaveable(
        saver = listSaver(save = { it.toList() }, restore = { it.toMutableStateList() }),
    ) { mutableStateListOf<String>() }
    var highlightId by remember { mutableStateOf<String?>(null) }
    var menuTargetId by rememberSaveable { mutableStateOf<String?>(null) }
    var jumpTargetId by remember { mutableStateOf<String?>(null) }
    var headerHeight by remember { mutableIntStateOf(0) }
    var dockHeight by remember { mutableIntStateOf(0) }
    var homeDockHeight by remember { mutableIntStateOf(0) }
    val allCaptures = state.captures
    val byId = remember(allCaptures) { allCaptures.associateBy(Capture::id) }
    val selected = selectedId?.let(byId::get)
    val menuCapture = menuTargetId?.let(byId::get)
    val deleteTarget = deleteSnapshot?.takeIf { target -> allCaptures.any { it.id == target.id } }
    val replyTarget = state.replyTo?.let(byId::get)
    val pushableIds = selectedIds.filter { id ->
        byId[id]?.let { !it.isLong && it.body.isNotBlank() } == true
    }
    val imeVisible = WindowInsets.isImeVisible
    var pendingLongOpen by remember { mutableStateOf(false) }
    val localLongId = state.longCapture?.captureId
    val filtered = remember(allCaptures, archivedOnly, pinnedOnly, state.archivePreviews, localLongId) {
        val projected = allCaptures.mapNotNull { item ->
            if (item.sessionOpen && item.id == localLongId) null
            else state.archivePreviews[item.id]?.let { preview ->
                item.copy(archived = preview.archived, archivedAtMicros = preview.archivedAtMicros)
            } ?: item
        }
        CaptureFilter(archivedOnly, pinnedOnly).applyTo(projected)
    }
    val headerVisible = selected == null
    val topPadding = with(density) { (if (headerHeight == 0) 76.dp else headerHeight.toDp()) + 4.dp }
    val dockPadding = with(density) { (if (dockHeight == 0) 164.dp else dockHeight.toDp()) + 6.dp }

    fun dismissInput() {
        focusManager.clearFocus()
        keyboard?.hide()
    }

    fun toggleExpand(id: String) {
        dismissInput()
        // Anchor the folded row by its top edge, not by whichever row happens to be first.
        val info = homeList.layoutInfo.visibleItemsInfo.firstOrNull { it.key == id }
        expandAnchor = ExpandAnchor(
            id = id,
            top = info?.offset ?: 0,
            firstIndex = homeList.firstVisibleItemIndex,
            firstOffset = homeList.firstVisibleItemScrollOffset,
        )
        if (id in expandedIds) expandedIds.remove(id) else expandedIds.add(id)
    }

    LaunchedEffect(expandedIds.toList()) {
        val anchor = expandAnchor ?: return@LaunchedEffect
        expandAnchor = null
        withFrameNanos { }
        withFrameNanos { }
        val info = homeList.layoutInfo.visibleItemsInfo.firstOrNull { it.key == anchor.id }
        if (info != null) {
            homeList.scrollBy((info.offset - anchor.top).toFloat())
        } else {
            homeList.scrollToItem(anchor.firstIndex, anchor.firstOffset)
        }
    }

    fun leaveSelection() {
        selecting = false
        selectedIds.clear()
    }

    fun enterSelection(capture: Capture) {
        dismissInput()
        selecting = true
        selectedIds.clear()
        selectedIds.add(capture.id)
    }

    fun jumpToReplyTarget(target: Capture) {
        if (archivedOnly != target.archived) archivedOnly = target.archived
        if (pinnedOnly && !target.pinned) pinnedOnly = false
        selectedId = null
        highlightId = null
        jumpTargetId = target.id
    }

    fun beginReply(capture: Capture) {
        if (state.busy || state.pending || !state.draftAvailable) return
        onReply(capture)
        requestReplyFocus = true
    }

    SideEffect {
        if (selected == null) selectedId = null
        if (deleteTarget == null) deleteSnapshot = null
        if (!state.pending) confirmRelease = false
    }

    LaunchedEffect(state.captures) {
        val ids = state.captures.mapTo(HashSet()) { it.id }
        selectedIds.retainAll { it in ids }
        if (selecting && selectedIds.isEmpty()) selecting = false
    }
    LaunchedEffect(jumpTargetId, filtered) {
        val id = jumpTargetId ?: return@LaunchedEffect
        val index = timelineItemIndex(filtered, id, ZoneId.systemDefault(), groupByDay = !archivedOnly)
        if (index == null) {
            jumpTargetId = null
            return@LaunchedEffect
        }
        homeList.animateScrollToItem(index)
        homeList.layoutInfo.visibleItemsInfo.firstOrNull { it.index == index }?.let { info ->
            val viewport = homeList.layoutInfo.viewportSize.height
            homeList.animateScrollBy((info.offset + info.size / 2 - viewport / 2).toFloat())
        }
        highlightId = id
        jumpTargetId = null
    }

    LaunchedEffect(pendingLongOpen, imeVisible) {
        if (pendingLongOpen && !imeVisible) {
            pendingLongOpen = false
            onOpenLong()
        }
    }
    LaunchedEffect(pendingLongOpen) {
        if (!pendingLongOpen) return@LaunchedEffect
        delay(350L)
        if (pendingLongOpen) {
            pendingLongOpen = false
            onOpenLong()
        }
    }

    LaunchedEffect(highlightId) {
        val id = highlightId ?: return@LaunchedEffect
        delay(2000L)
        if (highlightId == id) highlightId = null
    }

    LaunchedEffect(state.replyTo, state.editingId, requestReplyFocus) {
        if (requestReplyFocus && (state.replyTo != null || state.editingId != null)) {
            requestReplyFocus = false
            replyFocus.requestFocus()
            keyboard?.show()
        }
    }
    val long = state.longCapture
    if (state.longCaptureVisible && long != null) {
        LongCaptureScreen(
            state = long, onLeave = onLeaveLong, onDraft = onLongDraft, onAppend = onAppendLong,
            onFinish = onFinishLong, onCheck = onCheckLong, onForget = onForgetLong,
            onDeleteDraft = onDeleteLongDraft,
            connection = state.connection, onConnect = onConnect, onAuthorize = onAuthorize,
        )
        return
    }

    if (!ready && !state.snapshotLoaded && !state.cacheLoaded) {
        Column(
            Modifier.fillMaxSize()
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal)),
        ) {
            val startupMessage = state.message?.takeUnless { it in quietSuccessMessages }
                ?: if (state.longCaptureStorageFailed) "端末の long capture 記録を読み込めません。long capture 入力は停止しています。" else null
            if (startupMessage != null) {
                MessageBanner(
                    startupMessage,
                    Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    onDismiss = null,
                )
            }
            ConnectionStatus(state.connection, onConnect, onAuthorize, Modifier.weight(1f))
        }
        return
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
                duration = SnackbarDuration.Short,
            )
            if (result == SnackbarResult.ActionPerformed) undo(notice) else dismiss(notice)
        }
    }

    Box(Modifier.fillMaxSize().background(colors.background).imePadding()) {
        Box(
            Modifier.fillMaxSize()
                .then(if (backdrop.blurSupported) Modifier.moteBackdropSource(backdrop) else Modifier)
                .background(colors.background)
                .dismissInputOnTap { dismissInput() },
        ) {
            when {
                selected != null -> {
                    BackHandler(enabled = deleteTarget == null) { selectedId = null }
                    key(selected.id) {
                        CaptureDetail(
                            selected, byId, backdrop, topPadding, dockPadding,
                            onOpenReplyTarget = { target -> jumpToReplyTarget(target) },
                        )
                    }
                }
                else -> {
                    if (filtered.isEmpty() && (state.snapshotLoaded || state.cacheLoaded)) {
                        BoxWithSafeHorizontal(Modifier.fillMaxSize()) {
                            Column(
                                Modifier.align(Alignment.Center).padding(horizontal = 24.dp),
                                horizontalAlignment = Alignment.CenterHorizontally,
                                verticalArrangement = Arrangement.spacedBy(6.dp),
                            ) {
                                Text(
                                    when {
                                        !ready -> "再接続しています"
                                        pinnedOnly -> "ピン留めはありません"
                                        archivedOnly -> "アーカイブはありません"
                                        else -> "Inbox は空です"
                                    },
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = colors.onSurfaceVariant,
                                    textAlign = TextAlign.Center,
                                )
                                if (!ready) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                            }
                        }
                    } else {
                        key(archivedOnly, pinnedOnly) {
                            CaptureTimeline(
                                captures = filtered,
                                listState = homeList,
                                backdrop = backdrop,
                                topPadding = topPadding,
                                bottomPadding = dockPadding,
                                groupByDay = !archivedOnly,
                                enabled = canMutate,
                                updating = state.archivePreviews.keys,
                                archiveHaptics = preferences.archiveHaptics,
                                selecting = selecting,
                                selectedIds = selectedIds,
                                expandedIds = expandedIds,
                                replyTo = state.replyTo,
                                byId = byId,
                                highlightId = highlightId,
                                canReply = !state.busy && !state.pending && state.draftAvailable,
                                onSelect = { dismissInput(); selectedId = it.id },
                                onReply = { beginReply(it) },
                                onArchive = { dismissInput(); onArchive(it, !it.archived) },
                                onPin = { onPin(it, !it.pinned) },
                                onCopy = { copyCapture(context, it.body) },
                                onShare = { shareCapture(context, it.body) },
                                onDelete = { deleteSnapshot = it },
                                onEdit = { requestReplyFocus = true; onBeginEdit(it) },
                                onEnterSelection = { enterSelection(it) },
                                onToggleSelection = {
                                    if (it.id in selectedIds) selectedIds.remove(it.id) else selectedIds.add(it.id)
                                    if (selectedIds.isEmpty()) selecting = false
                                },
                                onToggleExpand = { toggleExpand(it.id) },
                                onOpenReplyTarget = { dismissInput(); jumpToReplyTarget(it) },
                                onMenu = { dismissInput(); menuTargetId = it.id },
                            )
                        }
                    }
                }
            }
        }

        if (selected == null) {
            AnimatedVisibility(
                visible = headerVisible,
                modifier = Modifier.align(Alignment.TopCenter),
                enter = fadeIn() + slideInVertically { -it / 2 },
                exit = fadeOut() + slideOutVertically { -it / 2 },
            ) {
                Box(Modifier.dismissInputOnTap { dismissInput() }) {
                    HomeHeader(
                        state = state,
                        onConnect = onConnect,
                        onAuthorize = onAuthorize,
                        onSync = onSync,
                        onOpenSettings = { dismissInput(); onOpenSettings() },
                        backdrop = backdrop,
                        modifier = Modifier.onSizeChanged { headerHeight = it.height },
                    )
                }
            }
            if (!headerVisible) {
                CachedReconnectChip(
                    state, onConnect, onAuthorize,
                    Modifier.align(Alignment.TopCenter).windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top)),
                )
            }
        } else {
            Box(Modifier.align(Alignment.TopCenter).dismissInputOnTap { dismissInput() }) {
                DetailHeader(
                    capture = selected,
                    canDelete = canMutate && selected.id !in state.archivePreviews,
                    onBack = { selectedId = null },
                    onDelete = { deleteSnapshot = selected },
                    backdrop = backdrop,
                    modifier = Modifier.onSizeChanged { headerHeight = it.height },
                )
            }
        }

        if (selected == null) {
            if (selecting) {
                val selectionHeight = with(LocalDensity.current) { homeDockHeight.toDp() }
                    .takeIf { it > 0.dp } ?: 164.dp
                Box(Modifier.align(Alignment.BottomCenter).height(selectionHeight)) {
                    SelectionDock(
                        count = pushableIds.size,
                        canPush = canMutate && pushableIds.isNotEmpty(),
                        onCancel = { leaveSelection() },
                        onPush = { confirmPush = true },
                        modifier = Modifier.align(Alignment.BottomCenter).fillMaxWidth(),
                    )
                }
            } else {
                HomeDock(
                    state = state,
                    ready = ready,
                    canMutate = canMutate,
                    archivedOnly = archivedOnly,
                    pinnedOnly = pinnedOnly,
                    replyTarget = replyTarget,
                    replyMissing = state.replyTo != null && replyTarget == null,
                    dockHeight = { height ->
                        if (!imeVisible) homeDockHeight = height
                        dockHeight = height
                    },
                    focusRequester = replyFocus,
                    onDraft = onDraft,
                    onSubmit = onSubmit,
                    editing = state.editingId != null,
                    onCancelEdit = onCancelEdit,
                    onArchivedOnly = {
                        archivedOnly = it
                        leaveSelection()
                    },
                    onPinnedOnly = {
                        pinnedOnly = it
                        leaveSelection()
                    },
                    onRelease = { confirmRelease = true },
                    onOpenLong = { dismissInput(); pendingLongOpen = true },
                    onCancelReply = onCancelReply,
                    onDeleteLongDraft = { confirmDeleteDraft = true },
                    modifier = Modifier.align(Alignment.BottomCenter),
                )
            }
        } else {
            DetailActions(
                capture = selected,
                canMutate = canMutate,
                inFlight = selected.id in state.archivePreviews,
                onCopy = { copyCapture(context, selected.body) },
                onShare = { shareCapture(context, selected.body) },
                onArchive = { onArchive(selected, !selected.archived) },
                onPin = { onPin(selected, !selected.pinned) },
                modifier = Modifier.align(Alignment.BottomCenter).onSizeChanged { dockHeight = it.height },
            )
        }

        // Notices sit below the header at the top of the screen, stacked with the
        // error banner so a transient confirmation never hides behind the dock.
        Column(
            Modifier.align(Alignment.TopCenter)
                .fillMaxWidth()
                .padding(top = topPadding + 4.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            val message = state.message?.takeUnless { it in quietSuccessMessages }
            if (message != null) {
                var dismissed by remember(message) { mutableStateOf(false) }
                if (!dismissed) {
                    MessageBanner(
                        message,
                        Modifier.padding(horizontal = 16.dp),
                        onDismiss = { dismissed = true },
                    )
                }
            } else if (state.longCaptureStorageFailed) {
                MessageBanner(
                    "端末の long capture 記録を読み込めません。long capture 入力は停止しています。",
                    Modifier.padding(horizontal = 16.dp),
                    onDismiss = null,
                )
            }
            SnackbarHost(
                hostState = snackbar,
                modifier = Modifier.padding(horizontal = 20.dp),
            ) { data ->
                val dismissState = rememberSwipeToDismissBoxState(
                    confirmValueChange = { value ->
                        if (value != SwipeToDismissBoxValue.Settled) {
                            data.dismiss()
                            true
                        } else false
                    },
                )
                SwipeToDismissBox(
                    state = dismissState,
                    modifier = Modifier.fillMaxWidth(),
                    enableDismissFromStartToEnd = true,
                    enableDismissFromEndToStart = true,
                    gesturesEnabled = true,
                    backgroundContent = {},
                ) {
                    Snackbar(
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 0.dp, vertical = 5.dp),
                        shape = MaterialTheme.shapes.medium,
                        containerColor = colors.surfaceContainerHigh,
                        contentColor = colors.onSurface,
                        action = { TextButton(onClick = data::performAction, enabled = canMutate) { Text("元に戻す") } },
                        dismissAction = {
                            IconButton(onClick = data::dismiss) { MoteIcon(MoteGlyph.Close, "閉じる", Modifier.size(18.dp)) }
                        },
                    ) { Text(data.visuals.message) }
                }
            }
        }
    }

    menuCapture?.let { target ->
        CaptureActionSheet(
            capture = target,
            enabled = canMutate,
            canReply = !state.busy && !state.pending && state.draftAvailable,
            onDismiss = { menuTargetId = null },
            onReply = { beginReply(target) },
            onEdit = { requestReplyFocus = true; onBeginEdit(target) },
            onDetail = { selectedId = target.id },
            onSelect = { enterSelection(target) },
            onPin = { onPin(target, !target.pinned) },
            onCopy = { copyCapture(context, target.body) },
            onShare = { shareCapture(context, target.body) },
            onArchive = { onArchive(target, !target.archived) },
            onDelete = { deleteSnapshot = target },
        )
    }

    if (deleteTarget != null) {
        DeleteCaptureDialog(
            deleteTarget,
            canMutate && deleteTarget.id !in state.archivePreviews,
            onCancel = { deleteSnapshot = null },
        ) {
            deleteSnapshot = null
            onDelete(deleteTarget)
        }
    }
    if (confirmDeleteDraft) {
        AlertDialog(
            onDismissRequest = { confirmDeleteDraft = false },
            icon = { MoteIcon(MoteGlyph.Delete, null, tint = colors.error) },
            text = { Text("削除しますか？") },
            confirmButton = {
                TextButton(
                    enabled = !state.busy,
                    colors = ButtonDefaults.textButtonColors(contentColor = colors.error),
                    onClick = {
                        confirmDeleteDraft = false
                        onDeleteLongDraft()
                    },
                ) { Text("削除") }
            },
            dismissButton = { TextButton(onClick = { confirmDeleteDraft = false }) { Text("キャンセル") } },
        )
    }
    if (confirmPush) {
        AlertDialog(
            onDismissRequest = { confirmPush = false },
            text = { Text("long capture に移動しますか？") },
            confirmButton = {
                TextButton(
                    enabled = canMutate && pushableIds.isNotEmpty(),
                    onClick = {
                        val ids = pushableIds.toList()
                        confirmPush = false
                        leaveSelection()
                        onPushToLong(ids)
                    },
                ) { Text("移動") }
            },
            dismissButton = { TextButton(onClick = { confirmPush = false }) { Text("キャンセル") } },
        )
    }
    if (confirmRelease && state.pending && state.draftAvailable) {
        AlertDialog(
            onDismissRequest = { confirmRelease = false },
            text = { Text("下書きの編集を再開しますか？") },
            confirmButton = {
                TextButton(enabled = !state.busy, onClick = { confirmRelease = false; onReleasePending() }) { Text("編集を再開") }
            },
            dismissButton = { TextButton(onClick = { confirmRelease = false }) { Text("キャンセル") } },
        )
    }
    BackHandler(enabled = selecting && selected == null) { leaveSelection() }
}

private val quietSuccessMessages = setOf(
    "Harmion に保存しました。",
    "削除しました。",
    "long capture を閉じました。",
    "下書きを削除しました。",
    "同期を要求しました。",
)

@Composable
private fun HomeHeader(
    state: MoteUiState,
    onConnect: () -> Unit,
    onAuthorize: () -> Unit,
    onSync: () -> Unit,
    onOpenSettings: () -> Unit,
    backdrop: MoteBackdrop,
    modifier: Modifier = Modifier,
) {
    val colors = MaterialTheme.colorScheme
    val ready = state.connection == MoteConnection.Ready
    val syncing = state.syncRequestBusy || state.sync.inFlight
    BlurredBar(
        backdrop,
        modifier.fillMaxWidth(),
        tint = colors.background.copy(alpha = if (backdrop.blurSupported) .60f else .96f),
    ) {
        Row(
            Modifier.fillMaxWidth()
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal))
                .heightIn(min = 54.dp)
                .padding(start = 10.dp, end = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            HeaderStatus(state, ready, onConnect, onAuthorize, onOpenSettings)
            Spacer(Modifier.width(2.dp))
            Text("Mote", style = MaterialTheme.typography.titleMedium, color = colors.onSurface)
            Spacer(Modifier.weight(1f))
            SyncButton(
                ready = ready,
                syncing = syncing,
                enabled = ready,
                onClick = onSync,
            )
            IconButton(onClick = onOpenSettings) {
                MoteIcon(MoteGlyph.Settings, "設定", tint = colors.onSurfaceVariant)
            }
        }
    }
}

@Composable
private fun HeaderStatus(
    state: MoteUiState,
    ready: Boolean,
    onConnect: () -> Unit,
    onAuthorize: () -> Unit,
    onOpenSettings: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    when {
        state.connection == MoteConnection.NeedsApproval -> {
            IconButton(onClick = onAuthorize) { MoteIcon(MoteGlyph.Link, "承認をリクエスト", tint = colors.tertiary) }
        }
        state.connection == MoteConnection.Loading || state.connection == MoteConnection.Approving -> {
            Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = colors.tertiary)
            }
        }
        !ready -> {
            IconButton(onClick = onConnect) { MoteIcon(MoteGlyph.Link, "再接続", tint = colors.tertiary) }
        }
        state.sync.lastError != null || state.sync.blockedCode != null || !state.sync.enabled || !state.sync.permitted -> {
            IconButton(onClick = onOpenSettings) {
                MoteIcon(MoteGlyph.Alert, "同期の状態", tint = colors.tertiary)
            }
        }
        else -> Spacer(Modifier.size(48.dp))
    }
}

@Composable
private fun SyncButton(
    ready: Boolean,
    syncing: Boolean,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val angle = remember { Animatable(0f) }
    LaunchedEffect(syncing) {
        if (syncing) {
            angle.snapTo(0f)
            while (true) {
                angle.animateTo(angle.value + 360f, tween(durationMillis = 900, easing = LinearEasing))
            }
        } else {
            angle.snapTo(0f)
        }
    }
    IconButton(
        onClick = { if (!syncing) onClick() },
        enabled = enabled,
    ) {
        MoteIcon(
            MoteGlyph.Sync,
            if (syncing) "同期中" else "今すぐ同期",
            Modifier.size(21.dp).rotate(angle.value),
            tint = if (syncing) colors.primary else if (ready) colors.onSurfaceVariant else colors.onSurfaceVariant.copy(alpha = .38f),
        )
    }
}

@Composable
private fun CachedReconnectChip(
    state: MoteUiState,
    onConnect: () -> Unit,
    onAuthorize: () -> Unit,
    modifier: Modifier = Modifier,
) {
    if (state.connection == MoteConnection.Ready) return
    val colors = MaterialTheme.colorScheme
    val busy = state.connection == MoteConnection.Loading || state.connection == MoteConnection.Approving
    val action = when (state.connection) {
        MoteConnection.NeedsApproval -> onAuthorize
        else -> onConnect
    }
    Surface(
        modifier = modifier.padding(top = 7.dp).clip(CircleShape)
            .then(if (busy) Modifier else Modifier.clickable(onClick = action)),
        shape = CircleShape,
        color = colors.surfaceContainerHigh.copy(alpha = .92f),
        contentColor = colors.onSurface,
    ) {
        Box(Modifier.padding(horizontal = 12.dp, vertical = 7.dp)) {
            when (state.connection) {
                MoteConnection.Loading, MoteConnection.Approving ->
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = colors.tertiary)
                MoteConnection.NeedsApproval ->
                    MoteIcon(MoteGlyph.Link, "承認をリクエスト", Modifier.size(17.dp), colors.tertiary)
                MoteConnection.Unavailable, MoteConnection.Failed ->
                    MoteIcon(MoteGlyph.Link, "再接続", Modifier.size(17.dp), colors.tertiary)
                MoteConnection.Ready -> Unit
            }
        }
    }
}

@Composable
private fun DetailHeader(
    capture: Capture,
    canDelete: Boolean,
    onBack: () -> Unit,
    onDelete: () -> Unit,
    backdrop: MoteBackdrop,
    modifier: Modifier = Modifier,
) {
    val colors = MaterialTheme.colorScheme
    BlurredBar(
        backdrop,
        modifier.fillMaxWidth(),
        tint = colors.background.copy(alpha = if (backdrop.blurSupported) .60f else .96f),
    ) {
        Row(
            Modifier.fillMaxWidth()
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal))
                .heightIn(min = 54.dp)
                .padding(start = 4.dp, end = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) { MoteIcon(MoteGlyph.Back, "戻る", tint = colors.onSurface) }
            Text(
                if (capture.isLong) "long capture" else "short capture",
                Modifier.weight(1f).semantics { heading() },
                style = MaterialTheme.typography.titleLarge,
                color = colors.onSurface,
            )
            IconButton(onClick = onDelete, enabled = canDelete) {
                MoteIcon(
                    MoteGlyph.Delete,
                    "削除",
                    tint = if (canDelete) colors.error else colors.onSurfaceVariant.copy(alpha = .38f),
                )
            }
        }
    }
}

@Composable
private fun MessageBanner(
    text: String,
    modifier: Modifier = Modifier,
    onDismiss: (() -> Unit)?,
) {
    val colors = MaterialTheme.colorScheme
    Surface(
        modifier = modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.medium,
        color = colors.surfaceContainerHigh.copy(alpha = .96f),
        contentColor = colors.onSurface,
        shadowElevation = 3.dp,
    ) {
        Row(
            Modifier.padding(start = 12.dp, end = 4.dp, top = 7.dp, bottom = 7.dp)
                .semantics { liveRegion = LiveRegionMode.Polite },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(7.dp),
        ) {
            MoteIcon(MoteGlyph.Alert, null, Modifier.size(16.dp), colors.secondary)
            Text(text, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
            if (onDismiss != null) {
                IconButton(onClick = onDismiss) { MoteIcon(MoteGlyph.Close, "閉じる", Modifier.size(16.dp)) }
            }
        }
    }
}

@Composable
private fun ConnectionStatus(connection: MoteConnection, onConnect: () -> Unit, onAuthorize: () -> Unit, modifier: Modifier) {
    when (connection) {
        MoteConnection.Loading -> CenteredState("Harmion に接続中", "この端末で下書きを続けられます。", modifier, loading = true) {
            TextButton(onClick = onConnect) { Text("再接続") }
        }
        MoteConnection.Approving -> CenteredState("承認を待っています", "Harmion でアクセスを許可してください。", modifier, loading = true) {
            TextButton(onClick = onAuthorize) { Text("承認をやり直す") }
        }
        MoteConnection.NeedsApproval -> CenteredState("Harmion とつなぐ", "Mote の capture だけにアクセスします。", modifier, mark = true) {
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
private fun BoxWithSafeHorizontal(modifier: Modifier, content: @Composable BoxScope.() -> Unit) {
    Box(modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal)), content = content)
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun HomeDock(
    state: MoteUiState,
    ready: Boolean,
    canMutate: Boolean,
    archivedOnly: Boolean,
    pinnedOnly: Boolean,
    replyTarget: Capture?,
    replyMissing: Boolean,
    dockHeight: (Int) -> Unit,
    focusRequester: androidx.compose.ui.focus.FocusRequester,
    onDraft: (String) -> Unit,
    onSubmit: () -> Unit,
    editing: Boolean,
    onCancelEdit: () -> Unit,
    onArchivedOnly: (Boolean) -> Unit,
    onPinnedOnly: (Boolean) -> Unit,
    onRelease: () -> Unit,
    onOpenLong: () -> Unit,
    onCancelReply: () -> Unit,
    onDeleteLongDraft: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = MaterialTheme.colorScheme
    val imeVisible = WindowInsets.isImeVisible
    val editingBody = if (editing) state.captures.firstOrNull { it.id == state.editingId }?.body else null
    val canSubmit = ready && state.draftAvailable && !state.busy && state.draft.isNotBlank() &&
        (!editing || state.draft != editingBody)
    val canOpenLong = state.longCaptureAvailable && !state.busy
    val canStartLong = canOpenLong && !state.pending
    val long = state.longCapture
    Box(modifier.fillMaxWidth().onSizeChanged { dockHeight(it.height) }) {
        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = 680.dp).fillMaxWidth()
                    .then(if (imeVisible) Modifier else Modifier.windowInsetsPadding(WindowInsets.navigationBars.only(WindowInsetsSides.Bottom)))
                    .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal))
                    .padding(horizontal = 20.dp, vertical = 7.dp),
                verticalArrangement = Arrangement.spacedBy(5.dp),
            ) {
                if (!imeVisible) {
                    if (long != null) {
                        LongDraftCard(
                            long, canOpenLong,
                            onClick = onOpenLong,
                            onLongClick = onDeleteLongDraft,
                        )
                    }
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                        Spacer(Modifier.weight(1f))
                        FilterRow(
                            ready = ready || state.cacheLoaded,
                            archivedOnly = archivedOnly,
                            pinnedOnly = pinnedOnly,
                            onArchivedOnly = onArchivedOnly,
                            onPinnedOnly = onPinnedOnly,
                        )
                    }
                }
                if (state.replyTo != null) {
                    ReplyComposerStrip(
                        target = replyTarget,
                        missing = replyMissing,
                        onCancel = onCancelReply,
                    )
                }
                if (state.pending && !state.busy) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text("送信結果を確認できません", Modifier.weight(1f), style = MaterialTheme.typography.labelMedium, color = colors.tertiary)
                        TextButton(onClick = onRelease, enabled = !state.busy) { Text("下書きを編集") }
                    }
                }
                MoteComposer(
                    state.draft,
                    onDraft,
                    placeholder = if (editing) "short capture を編集…" else null,
                    accessibilityLabel = if (editing) "short capture を編集" else "short capture を書く",
                    enabled = state.draftAvailable,
                    readOnly = state.pending || state.busy,
                    focusRequester = focusRequester,
                    translucent = false,
                    actionCount = if (editing || long == null) 2 else 1,
                ) {
                    AnimatedContent(
                        targetState = editing,
                        transitionSpec = { fadeIn(tween(160)) togetherWith fadeOut(tween(120)) },
                        label = "composer edit action",
                    ) { isEditing ->
                        if (isEditing) {
                            IconButton(onClick = onCancelEdit, enabled = !state.busy && !state.pending) {
                                MoteIcon(MoteGlyph.Close, "編集をやめる", Modifier.size(20.dp), colors.onSurfaceVariant)
                            }
                        } else if (long == null) {
                            IconButton(onClick = onOpenLong, enabled = canStartLong) {
                                MoteIcon(
                                    MoteGlyph.Expand,
                                    "long capture を書く",
                                    Modifier.size(20.dp),
                                    colors.onSurfaceVariant.copy(alpha = if (canStartLong) 1f else .45f),
                                )
                            }
                        }
                    }
                    MoteSendButton(canSubmit, if (editing) "保存" else if (state.pending) "再送" else "送信", onSubmit)
                }
            }
        }
    }
}

@Composable
private fun LongDraftCard(
    state: LongCaptureUi,
    enabled: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    Surface(
        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(18.dp))
            .combinedClickable(
                enabled = enabled,
                onClickLabel = "long capture を再開",
                onLongClickLabel = "下書きの操作",
                onClick = onClick,
                onLongClick = onLongClick,
            ),
        shape = RoundedCornerShape(18.dp),
        color = colors.surfaceContainerHigh,
    ) {
        Row(
            Modifier.padding(start = 13.dp, end = 13.dp, top = 8.dp, bottom = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            MoteIcon(MoteGlyph.Edit, null, Modifier.size(17.dp), colors.primary)
            Column(Modifier.weight(1f)) {
                Text("long capture の下書き", style = MaterialTheme.typography.labelLarge)
                val preview = state.draft.ifBlank { state.body }.trimStart()
                if (preview.isNotEmpty()) {
                    Text(
                        preview, color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall,
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun FilterRow(
    ready: Boolean,
    archivedOnly: Boolean,
    pinnedOnly: Boolean,
    onArchivedOnly: (Boolean) -> Unit,
    onPinnedOnly: (Boolean) -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    SingleChoiceSegmentedButtonRow {
        listOf(MoteGlyph.Inbox to "Inbox", MoteGlyph.Archive to "Archived").forEachIndexed { index, (glyph, label) ->
            SegmentedButton(
                selected = archivedOnly == (index == 1), onClick = { onArchivedOnly(index == 1) },
                enabled = ready, shape = SegmentedButtonDefaults.itemShape(index, 2),
                colors = SegmentedButtonDefaults.colors(
                    activeContainerColor = colors.primaryContainer, activeContentColor = colors.onPrimaryContainer,
                    activeBorderColor = colors.outlineVariant, inactiveContainerColor = colors.surfaceContainerLow,
                    inactiveContentColor = colors.onSurfaceVariant, inactiveBorderColor = colors.outlineVariant,
                ),
                icon = {},
            ) { MoteIcon(glyph, label, Modifier.size(19.dp)) }
        }
    }
    FilledTonalIconToggleButton(
        checked = pinnedOnly, onCheckedChange = onPinnedOnly, enabled = ready,
        colors = IconButtonDefaults.filledTonalIconToggleButtonColors(
            containerColor = colors.surfaceContainer, contentColor = colors.onSurfaceVariant,
            checkedContainerColor = colors.primaryContainer, checkedContentColor = colors.primary,
        ),
    ) { MoteIcon(MoteGlyph.Pin, "ピン留めのみ", Modifier.size(19.dp)) }
}

@Composable
private fun ReplyComposerStrip(target: Capture?, missing: Boolean, onCancel: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(15.dp),
        color = colors.surfaceContainerHigh,
    ) {
        Row(
            Modifier.padding(start = 11.dp, end = 3.dp, top = 3.dp, bottom = 3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(7.dp),
        ) {
            Box(Modifier.width(2.dp).height(24.dp).background(colors.primary.copy(alpha = .6f), CircleShape))
            MoteIcon(MoteGlyph.Reply, null, Modifier.size(14.dp), colors.primary)
            Text(
                if (missing || target == null) "元の capture は見つかりません" else previewOf(target.body),
                modifier = Modifier.weight(1f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.labelSmall,
                color = colors.onSurfaceVariant,
            )
            IconButton(onClick = onCancel, modifier = Modifier.size(34.dp)) {
                MoteIcon(MoteGlyph.Close, "返信をやめる", Modifier.size(16.dp), colors.onSurfaceVariant)
            }
        }
    }
}

private data class ExpandAnchor(val id: String, val top: Int, val firstIndex: Int, val firstOffset: Int)

@Composable
private fun CaptureTimeline(
    captures: List<Capture>,
    listState: LazyListState,
    backdrop: MoteBackdrop,
    topPadding: Dp,
    bottomPadding: Dp,
    groupByDay: Boolean,
    enabled: Boolean,
    updating: Set<String>,
    archiveHaptics: Boolean,
    selecting: Boolean,
    selectedIds: List<String>,
    expandedIds: List<String>,
    replyTo: String?,
    byId: Map<String, Capture>,
    highlightId: String?,
    canReply: Boolean,
    onSelect: (Capture) -> Unit,
    onReply: (Capture) -> Unit,
    onArchive: (Capture) -> Unit,
    onPin: (Capture) -> Unit,
    onCopy: (Capture) -> Unit,
    onShare: (Capture) -> Unit,
    onDelete: (Capture) -> Unit,
    onEdit: (Capture) -> Unit,
    onEnterSelection: (Capture) -> Unit,
    onToggleSelection: (Capture) -> Unit,
    onToggleExpand: (Capture) -> Unit,
    onOpenReplyTarget: (Capture) -> Unit,
    onMenu: (Capture) -> Unit,
) {
    val zone = ZoneId.systemDefault()
    val today = LocalDate.now(zone)
    val groups: List<Pair<LocalDate?, List<Capture>>> = remember(captures, zone, groupByDay) {
        if (groupByDay) {
            captures.asReversed().groupBy { it.createdAtMicros?.atZone(zone)?.toLocalDate() }.toList()
        } else {
            listOf(null to captures.asReversed())
        }
    }
    val gaps = remember(captures, zone, groupByDay) {
        val result = HashMap<String, Long?>()
        if (groupByDay) {
            var previous: Long? = null
            var previousDay: LocalDate? = null
            for (capture in captures) {
                val time = capture.createdAtMicros
                val day = time?.atZone(zone)?.toLocalDate()
                result[capture.id] = if (time != null && previous != null && day == previousDay) time - previous else null
                if (time != null) {
                    previous = time
                    previousDay = day
                } else {
                    previous = null
                    previousDay = null
                }
            }
        }
        result
    }
    val latest = captures.lastOrNull()?.id
    val followLatest = listState.firstVisibleItemIndex == 0 &&
        listState.firstVisibleItemScrollOffset == 0 && !listState.isScrollInProgress
    LaunchedEffect(latest) {
        if (followLatest) listState.scrollToItem(0)
    }
    Box(
        Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background),
        contentAlignment = Alignment.TopCenter,
    ) {
        Box(
            Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal)),
            contentAlignment = Alignment.TopCenter,
        ) {
            LazyColumn(
                Modifier.widthIn(max = 680.dp).fillMaxSize(),
                state = listState,
                reverseLayout = true,
                contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = topPadding, bottom = bottomPadding),
                verticalArrangement = Arrangement.spacedBy(6.dp, Alignment.Bottom),
            ) {
                groups.forEach { (day, entries) ->
                    items(entries, key = Capture::id, contentType = { "capture" }) { capture ->
                        CaptureRow(
                            capture = capture,
                            enabled = enabled && capture.id !in updating,
                            canReply = canReply && capture.id !in updating,
                            archiveHaptics = archiveHaptics,
                            gapBeforeMicros = gaps[capture.id],
                            selecting = selecting,
                            selected = capture.id in selectedIds,
                            expanded = capture.id in expandedIds,
                            isReplyTarget = replyTo == capture.id,
                            replyTarget = capture.replyTo?.let(byId::get),
                            highlighted = capture.id == highlightId,
                            onSelect = onSelect,
                            onReply = onReply,
                            onArchive = onArchive,
                            onPin = onPin,
                            onCopy = onCopy,
                            onShare = onShare,
                            onDelete = onDelete,
                            onEdit = onEdit,
                            onEnterSelection = onEnterSelection,
                            onToggleSelection = onToggleSelection,
                            onToggleExpand = onToggleExpand,
                            onOpenReplyTarget = onOpenReplyTarget,
                            onMenu = onMenu,
                        )
                    }
                    if (groupByDay) {
                        item(key = "day:$day", contentType = "day") { TimelineDay(day, today) }
                    }
                }
            }
        }
    }
}

private fun timelineItemIndex(
    captures: List<Capture>,
    targetId: String,
    zone: ZoneId,
    groupByDay: Boolean,
): Int? {
    if (!groupByDay) {
        val index = captures.asReversed().indexOfFirst { it.id == targetId }
        return index.takeIf { it >= 0 }
    }
    val days = captures.asReversed().groupBy { it.createdAtMicros?.atZone(zone)?.toLocalDate() }
    var index = 0
    for ((_, entries) in days) {
        for (capture in entries) {
            if (capture.id == targetId) return index
            index++
        }
        index++
    }
    return null
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
    capture: Capture,
    enabled: Boolean,
    canReply: Boolean,
    archiveHaptics: Boolean,
    gapBeforeMicros: Long?,
    selecting: Boolean,
    selected: Boolean,
    expanded: Boolean,
    highlighted: Boolean,
    isReplyTarget: Boolean,
    replyTarget: Capture?,
    onSelect: (Capture) -> Unit,
    onReply: (Capture) -> Unit,
    onArchive: (Capture) -> Unit,
    onPin: (Capture) -> Unit,
    onCopy: (Capture) -> Unit,
    onShare: (Capture) -> Unit,
    onDelete: (Capture) -> Unit,
    onEdit: (Capture) -> Unit,
    onEnterSelection: (Capture) -> Unit,
    onToggleSelection: (Capture) -> Unit,
    onToggleExpand: (Capture) -> Unit,
    onOpenReplyTarget: (Capture) -> Unit,
    onMenu: (Capture) -> Unit,
) {
    var foldable by remember(capture.id, capture.body) { mutableStateOf(false) }
    val colors = MaterialTheme.colorScheme
    val highlightAmount by animateFloatAsState(
        if (highlighted) 1f else 0f,
        tween(durationMillis = 260),
        label = "reply highlight",
    )
    val haptic = LocalHapticFeedback.current
    val layoutDirection = LocalLayoutDirection.current
    // Reply is a left swipe and archiving is a right swipe. SwipeToDismissBoxValue is
    // physical, so resolve it per layout direction.
    val archiveDirection = if (layoutDirection == LayoutDirection.Ltr) SwipeToDismissBoxValue.StartToEnd else SwipeToDismissBoxValue.EndToStart
    val replyDirection = if (layoutDirection == LayoutDirection.Ltr) SwipeToDismissBoxValue.EndToStart else SwipeToDismissBoxValue.StartToEnd
    val swipe = remember(capture.id) {
        SwipeToDismissBoxState(SwipeToDismissBoxValue.Settled, positionalThreshold = { width -> width * .32f })
    }
    LaunchedEffect(swipe, swipe.settledValue) {
        when (swipe.settledValue) {
            replyDirection -> {
                if (canReply) onReply(capture)
                swipe.reset()
            }
            archiveDirection -> {
                if (enabled) {
                    if (archiveHaptics) haptic.performHapticFeedback(HapticFeedbackType.Confirm)
                    onArchive(capture)
                }
                swipe.reset()
            }
            else -> Unit
        }
    }
    val replyArmed = swipe.targetValue == replyDirection
    val archiveArmed = swipe.targetValue == archiveDirection
    val rawOffset = runCatching { swipe.requireOffset() }.getOrDefault(0f)
    val draggingToReply = if (layoutDirection == LayoutDirection.Ltr) rawOffset < 0f else rawOffset > 0f
    val dragging = kotlin.math.abs(rawOffset) > 0.5f
    val showReply = dragging && (replyArmed || (!archiveArmed && swipe.progress > .01f && draggingToReply))
    val showArchive = dragging && (archiveArmed || (!replyArmed && swipe.progress > .01f && !draggingToReply))
    val showSwipeBackground = showReply || showArchive
    val swipeColor by animateColorAsState(
        when {
            showReply -> colors.primaryContainer
            showArchive -> colors.secondaryContainer
            else -> Color.Transparent
        },
        MaterialTheme.motionScheme.fastEffectsSpec(),
        label = "swipe action",
    )
    val shape = if (capture.isLong) RoundedCornerShape(18.dp) else
        RoundedCornerShape(topStart = 17.dp, topEnd = 17.dp, bottomEnd = 7.dp, bottomStart = 17.dp)
    val preview = remember(capture.body, capture.isLong) {
        capture.body.trimStart().ifBlank { "（本文なし）" }
    }
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val gutter = 44.dp
        val bubbleMaxWidth = (maxWidth - gutter - 6.dp) * MaxTextWidthFraction
        Box(
            Modifier.fillMaxWidth().padding(top = gapPadding(gapBeforeMicros)),
            contentAlignment = Alignment.BottomEnd,
        ) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.Bottom,
            ) {
                CaptureTimeGutter(capture, selected, Modifier.width(gutter))
                Box(Modifier.widthIn(max = bubbleMaxWidth)) {
                    SwipeToDismissBox(
                state = swipe,
                modifier = Modifier.clip(shape),
                enableDismissFromStartToEnd = enabled && !selecting,
                enableDismissFromEndToStart = canReply && !selecting,
                gesturesEnabled = !selecting,
                backgroundContent = {
                    if (showSwipeBackground) {
                        Box(
                            Modifier.fillMaxSize().background(swipeColor).padding(horizontal = 15.dp).clearAndSetSemantics {},
                            contentAlignment = if (showReply) Alignment.CenterEnd else Alignment.CenterStart,
                        ) {
                            val tint = if (showReply) colors.onPrimaryContainer else colors.onSecondaryContainer
                            MoteIcon(
                                if (showReply) MoteGlyph.Reply else if (capture.archived) MoteGlyph.Restore else MoteGlyph.Archive,
                                if (showReply) "返信" else archiveLabel(capture),
                                Modifier.size(18.dp),
                                tint,
                            )
                        }
                    }
                },
            ) {
                Box(contentAlignment = Alignment.CenterEnd) {
                    Surface(
                        modifier = Modifier
                            .then(if (capture.isLong) Modifier.fillMaxWidth() else Modifier.widthIn(max = bubbleMaxWidth))
                            .clip(shape)
                            .combinedClickable(
                                enabled = !(selecting && capture.isLong),
                                onClickLabel = when {
                                    selecting -> "選択"
                                    capture.isLong -> "long capture を開く"
                                    expanded -> "詳細を開く"
                                    foldable -> "全文を表示"
                                    else -> "short capture を開く"
                                },
                                onClick = {
                                    when {
                                        selecting -> if (!capture.isLong) onToggleSelection(capture)
                                        capture.isLong -> onSelect(capture)
                                        expanded -> onSelect(capture)
                                        foldable -> onToggleExpand(capture)
                                        else -> onSelect(capture)
                                    }
                                },
                                onLongClickLabel = if (selecting) "選択を切り替える" else "capture の操作",
                                onLongClick = {
                                    if (selecting) {
                                        if (!capture.isLong) onToggleSelection(capture)
                                    } else {
                                        haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                                        onMenu(capture)
                                    }
                                },
                            )
                            .then(if (isReplyTarget) Modifier.border(1.5.dp, colors.primary, shape) else Modifier),
                        shape = shape,
                        color = when {
                            selecting && selected -> colors.primaryContainer
                            capture.isLong -> colors.surfaceContainer
                            else -> colors.surfaceContainerHigh
                        },
                        border = when {
                            highlighted || highlightAmount > .01f -> BorderStroke(1.5.dp, colors.primary.copy(alpha = highlightAmount))
                            capture.isLong -> BorderStroke(1.dp, colors.outlineVariant)
                            else -> null
                        },
                    ) {
                        Column(
                            Modifier.padding(horizontal = 12.dp, vertical = if (capture.isLong) 10.dp else 7.dp),
                            verticalArrangement = Arrangement.spacedBy(4.dp),
                        ) {
                            if (capture.isLong) {
                                Text(
                                    if (capture.sessionOpen) "long capture・下書き" else "long capture",
                                    color = colors.primary, style = MaterialTheme.typography.labelSmall,
                                )
                            }
                            if (capture.replyTo != null) {
                                ReplyQuote(
                                    target = replyTarget,
                                    onOpen = if (selecting) null else replyTarget?.let { target -> { onOpenReplyTarget(target) } },
                                )
                            }
                            if (capture.isLong) {
                                Text(
                                    preview,
                                    style = MaterialTheme.typography.bodyMedium,
                                    maxLines = 3,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            } else {
                                Text(
                                    text = preview.ifBlank { "（本文なし）" },
                                    style = MaterialTheme.typography.bodyMedium,
                                    maxLines = if (expanded) Int.MAX_VALUE else CollapsedShortLines,
                                    overflow = if (expanded) TextOverflow.Clip else TextOverflow.Ellipsis,
                                    onTextLayout = { layout -> if (!expanded) foldable = layout.hasVisualOverflow },
                                )
                                if (!selecting) {
                                    when {
                                        expanded -> {
                                            TextButton(
                                                onClick = { onToggleExpand(capture) },
                                                modifier = Modifier.align(Alignment.End).heightIn(min = 28.dp),
                                                contentPadding = PaddingValues(horizontal = 6.dp, vertical = 0.dp),
                                            ) {
                                                MoteIcon(MoteGlyph.Fold, null, Modifier.size(15.dp))
                                                Spacer(Modifier.width(4.dp))
                                                Text("折りたたむ", style = MaterialTheme.typography.labelSmall)
                                            }
                                        }
                                        foldable -> {
                                            IconButton(
                                                onClick = { onToggleExpand(capture) },
                                                modifier = Modifier.align(Alignment.End).size(28.dp),
                                            ) { MoteIcon(MoteGlyph.Unfold, "全文を表示", Modifier.size(16.dp), colors.primary) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        }
    }
}
}

@Composable
private fun ReplyQuote(target: Capture?, onOpen: (() -> Unit)?) {
    val colors = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth()
            .heightIn(min = 44.dp)
            .clip(RoundedCornerShape(10.dp))
            .then(if (onOpen != null) Modifier.clickable(onClick = onOpen) else Modifier)
            .padding(horizontal = 2.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            Modifier.size(28.dp).clip(CircleShape).background(colors.primaryContainer.copy(alpha = .45f), CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            MoteIcon(
                MoteGlyph.Reply,
                if (target == null) "返信元が見つかりません" else "元のメッセージを表示",
                Modifier.size(15.dp),
                if (target == null) colors.onSurfaceVariant.copy(alpha = .6f) else colors.primary,
            )
        }
        Text(
            text = if (target == null) "元の capture は見つかりません" else previewOf(target.body),
            modifier = Modifier.weight(1f),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.labelSmall,
            color = if (target == null) colors.onSurfaceVariant.copy(alpha = .7f) else colors.onSurfaceVariant,
        )
    }
}

@Composable
private fun CaptureTimeGutter(capture: Capture, selected: Boolean, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    val time = capture.createdAtMicros?.atZone(ZoneId.systemDefault())?.format(shortTime) ?: "—"
    val markers = capture.pinned || capture.replyTo != null || selected
    Column(
        modifier.padding(bottom = 7.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        if (markers) {
            Row(horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.CenterVertically) {
                if (capture.pinned) MoteIcon(MoteGlyph.Pin, "ピン留め済み", Modifier.size(9.dp), colors.primary)
                if (capture.replyTo != null) MoteIcon(MoteGlyph.Reply, "返信", Modifier.size(9.dp), colors.primary)
                if (selected) MoteIcon(MoteGlyph.Check, "選択中", Modifier.size(9.dp), colors.primary)
            }
        }
        Text(time, color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall, maxLines = 1)
    }
}

@Composable
private fun CaptureMetadata(capture: Capture) {
    val date = capture.createdAtMicros?.atZone(ZoneId.systemDefault())
    val time = date?.format(detailTime) ?: "日時不明"
    val labels = buildList {
        add(time)
        if (capture.sessionOpen) add("下書き")
        if (capture.pinned) add("ピン留め済み")
    }
    Text(labels.joinToString("  ·  "), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

@Composable
private fun CaptureDetail(
    capture: Capture,
    byId: Map<String, Capture>,
    backdrop: MoteBackdrop,
    topPadding: Dp,
    bottomPadding: Dp,
    onOpenReplyTarget: (Capture) -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val body = capture.body.ifBlank { "（本文なし）" }
    val slices = remember(body, capture.isLong) { if (capture.isLong) threadSlices(body) else emptyList() }
    val replyTarget = capture.replyTo?.let(byId::get)
    Box(
        Modifier.fillMaxSize().background(colors.background)
            .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal)),
        contentAlignment = Alignment.TopCenter,
    ) {
        SelectionContainer(Modifier.widthIn(max = 680.dp).fillMaxSize()) {
            LazyColumn(
                contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = topPadding, bottom = bottomPadding),
            ) {
                item(key = "metadata") {
                    Column(Modifier.padding(bottom = 14.dp)) { CaptureMetadata(capture) }
                }
                if (capture.replyTo != null) {
                    item(key = "reply") {
                        Box(Modifier.padding(bottom = 14.dp)) {
                            ReplyQuote(
                                target = replyTarget,
                                onOpen = replyTarget?.let { target -> { onOpenReplyTarget(target) } },
                            )
                        }
                    }
                }
                if (capture.isLong) {
                    items(slices, key = { it.start }) { slice -> LongCaptureThreadSlice(body, slice) }
                } else {
                    item(key = "body") { Text(body, style = MaterialTheme.typography.bodyLarge, color = colors.onSurface) }
                }
            }
        }
    }
}

@Composable
private fun DetailActions(
    capture: Capture,
    canMutate: Boolean,
    inFlight: Boolean,
    onCopy: () -> Unit,
    onShare: () -> Unit,
    onArchive: () -> Unit,
    onPin: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = MaterialTheme.colorScheme
    val enabled = canMutate && !inFlight
    Box(modifier.fillMaxWidth().background(colors.surfaceContainer)) {
        Row(
            Modifier.fillMaxWidth()
                .windowInsetsPadding(WindowInsets.navigationBars.only(WindowInsetsSides.Bottom))
                    .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal))
                .padding(horizontal = 16.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            DetailAction(MoteGlyph.Archive, if (capture.archived) "Inbox に戻す" else "アーカイブ", enabled, onArchive)
            DetailAction(MoteGlyph.Share, "共有", capture.body.isNotEmpty(), onShare)
            DetailAction(MoteGlyph.Pin, if (capture.pinned) "ピン留め解除" else "ピン留め", enabled, onPin)
            DetailAction(MoteGlyph.Copy, "コピー", capture.body.isNotEmpty(), onCopy)
        }
    }
}

@Composable
private fun RowScope.DetailAction(glyph: MoteGlyph, label: String, enabled: Boolean, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Column(
        Modifier.weight(1f).clip(RoundedCornerShape(14.dp)).clickable(enabled = enabled, onClick = onClick)
            .padding(vertical = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        MoteIcon(glyph, null, Modifier.size(20.dp), if (enabled) colors.onSurfaceVariant else colors.onSurfaceVariant.copy(alpha = .38f))
        Text(
            label,
            style = MaterialTheme.typography.labelSmall,
            color = if (enabled) colors.onSurface else colors.onSurfaceVariant.copy(alpha = .38f),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            textAlign = TextAlign.Center,
        )
    }
}

@Composable
private fun SelectionDock(
    count: Int,
    canPush: Boolean,
    onCancel: () -> Unit,
    onPush: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = MaterialTheme.colorScheme
    Box(
        modifier.fillMaxWidth().background(
            Brush.verticalGradient(listOf(Color.Transparent, colors.background)),
        ),
        contentAlignment = Alignment.BottomCenter,
    ) {
        Row(
            Modifier.fillMaxWidth()
                .windowInsetsPadding(WindowInsets.navigationBars.only(WindowInsetsSides.Bottom))
                    .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal))
                .heightIn(min = 62.dp)
                .padding(start = 4.dp, end = 10.dp, bottom = 7.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            IconButton(onClick = onCancel) { MoteIcon(MoteGlyph.Close, "選択をやめる") }
            Text("${count}件", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            TextButton(onClick = onPush, enabled = canPush) {
                MoteIcon(MoteGlyph.Expand, null, Modifier.size(17.dp))
                Spacer(Modifier.width(5.dp))
                Text("long capture へ")
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun CaptureActionSheet(
    capture: Capture,
    enabled: Boolean,
    canReply: Boolean,
    onDismiss: () -> Unit,
    onReply: () -> Unit,
    onEdit: () -> Unit,
    onDetail: () -> Unit,
    onSelect: () -> Unit,
    onPin: () -> Unit,
    onCopy: () -> Unit,
    onShare: () -> Unit,
    onArchive: () -> Unit,
    onDelete: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var acting by remember { mutableStateOf(false) }
    fun act(action: () -> Unit) {
        if (acting) return
        acting = true
        onDismiss()
        action()
    }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        sheetMaxWidth = 560.dp,
        scrimColor = Color.Black.copy(alpha = .45f),
        containerColor = colors.surfaceContainer,
        dragHandle = { androidx.compose.material3.BottomSheetDefaults.DragHandle() },
    ) {
        Column(
            Modifier.fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(bottom = 8.dp),
        ) {
            Text(
                previewOf(capture.body),
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 10.dp),
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodyMedium,
                color = colors.onSurfaceVariant,
            )
            SheetAction(MoteGlyph.Reply, "返信", canReply) { act(onReply) }
            if (!capture.isLong) {
                SheetAction(MoteGlyph.Edit, "編集", enabled) { act(onEdit) }
            }
            SheetAction(MoteGlyph.Copy, "コピー", capture.body.isNotEmpty()) { act(onCopy) }
            SheetAction(MoteGlyph.Share, "共有", capture.body.isNotEmpty()) { act(onShare) }
            HorizontalDivider(Modifier.padding(horizontal = 20.dp, vertical = 4.dp), color = colors.outlineVariant)
            SheetAction(MoteGlyph.Expand, "詳細") { act(onDetail) }
            if (!capture.isLong) {
                SheetAction(MoteGlyph.Check, "選択") { act(onSelect) }
            }
            HorizontalDivider(Modifier.padding(horizontal = 20.dp, vertical = 4.dp), color = colors.outlineVariant)
            SheetAction(MoteGlyph.Pin, pinLabel(capture), enabled) { act(onPin) }
            SheetAction(if (capture.archived) MoteGlyph.Restore else MoteGlyph.Archive, archiveLabel(capture), enabled) { act(onArchive) }
            HorizontalDivider(Modifier.padding(horizontal = 20.dp, vertical = 4.dp), color = colors.outlineVariant)
            SheetAction(
                MoteGlyph.Delete,
                if (capture.sessionOpen) "下書きを削除…" else "削除…",
                enabled,
                destructive = true,
            ) { act(onDelete) }
        }
    }
}

@Composable
private fun SheetAction(
    glyph: MoteGlyph,
    label: String,
    enabled: Boolean = true,
    destructive: Boolean = false,
    onClick: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val tint = when {
        destructive && enabled -> colors.error
        else -> colors.onSurfaceVariant.copy(alpha = if (enabled) 1f else .38f)
    }
    val contentColor = when {
        !enabled -> colors.onSurface.copy(alpha = .38f)
        destructive -> colors.error
        else -> colors.onSurface
    }
    Row(
        Modifier.fillMaxWidth()
            .heightIn(min = 46.dp)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        MoteIcon(glyph, null, Modifier.size(20.dp), tint)
        Text(label, style = MaterialTheme.typography.bodyLarge, color = contentColor)
    }
}

@Composable
private fun DeleteCaptureDialog(capture: Capture, enabled: Boolean, onCancel: () -> Unit, onConfirm: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    AlertDialog(
        onDismissRequest = onCancel,
        icon = { MoteIcon(MoteGlyph.Delete, null, tint = colors.error) },
        text = { Text("削除しますか？") },
        confirmButton = {
            Button(
                onClick = onConfirm,
                enabled = enabled,
                colors = ButtonDefaults.buttonColors(containerColor = colors.errorContainer, contentColor = colors.onErrorContainer),
            ) { Text("削除") }
        },
        dismissButton = { TextButton(onClick = onCancel) { Text("キャンセル") } },
    )
}

@Composable
private fun Modifier.dismissInputOnTap(onDismiss: () -> Unit): Modifier {
    return pointerInput(Unit) {
        detectTapGestures { onDismiss() }
    }
}

private fun previewOf(body: String): String {
    val singleLine = body.trim().replace('\n', ' ').replace('\r', ' ')
    if (singleLine.length <= 72) return singleLine.ifBlank { "（本文なし）" }
    return singleLine.take(if (singleLine[72].isLowSurrogate()) 71 else 72) + "…"
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
