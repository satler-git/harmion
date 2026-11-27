package org.harmion.mote

import android.content.ClipData
import android.content.ClipDescription
import android.os.Build
import android.os.PersistableBundle
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun LongCaptureScreen(
    state: LongCaptureUi,
    onLeave: () -> Unit,
    onDraft: (String) -> Unit,
    onAppend: () -> Unit,
    onFinish: () -> Unit,
    onCheck: () -> Unit,
    onForget: () -> Unit,
    onDeleteDraft: () -> Unit,
    connection: MoteConnection,
    onConnect: () -> Unit,
    onAuthorize: () -> Unit,
) {
    var menuOpen by remember { mutableStateOf(false) }
    var confirmForget by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()
    val slices = remember(state.body) { threadSlices(state.body) }
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    val canEdit = !state.busy && !state.pending
    val canFinish = canEdit && (state.canUseNode || (state.canFinishLocally && state.draft.isBlank()))
    val colors = MaterialTheme.colorScheme
    var followFromBody by remember { mutableStateOf<String?>(null) }

    fun requestAppendOrCheck(action: () -> Unit) {
        // Only follow an explicit append/check if the reader was already at the end.
        followFromBody = state.body.takeIf { !listState.canScrollForward && !listState.isScrollInProgress }
        action()
    }

    LaunchedEffect(listState) {
        listState.interactionSource.interactions.collect { interaction ->
            if (interaction is DragInteraction.Start) followFromBody = null
        }
    }
    LaunchedEffect(state.body) {
        val previous = followFromBody ?: return@LaunchedEffect
        if (state.body.length > previous.length && state.body.startsWith(previous)) {
            // Wait for the list to lay out the new slices before scrolling to the footer.
            snapshotFlow { listState.layoutInfo.totalItemsCount }.first { it == slices.size + 1 }
            val shouldFollow = followFromBody != null && !listState.isScrollInProgress
            followFromBody = null
            if (shouldFollow) listState.animateScrollToItem(slices.size)
        } else {
            followFromBody = null
        }
    }

    BackHandler(enabled = !confirmForget && !confirmDelete && !menuOpen, onBack = onLeave)
    Scaffold(
        modifier = Modifier.imePadding(),
        containerColor = colors.background,
        topBar = {
            Surface(color = colors.background) {
                Column(
                    Modifier.fillMaxWidth().windowInsetsPadding(
                        WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Top),
                    ),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Row(
                        modifier = Modifier.widthIn(max = ThreadMaxWidth).fillMaxWidth()
                            .heightIn(min = 54.dp)
                            .padding(start = 8.dp, end = 20.dp),
                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        IconButton(onClick = onLeave) {
                            MoteIcon(MoteGlyph.Back, contentDescription = "中断して戻る")
                        }
                        Text("long capture", Modifier.weight(1f).semantics { heading() }, style = MaterialTheme.typography.titleLarge)
                        Box {
                            IconButton(onClick = { menuOpen = true }) {
                                MoteIcon(MoteGlyph.More, contentDescription = "long capture のその他の操作")
                            }
                            DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                                if (state.body.isNotEmpty()) {
                                    DropdownMenuItem(
                                        text = { Text("保存済みの本文をすべてコピー") },
                                        leadingIcon = { MoteIcon(MoteGlyph.Copy, contentDescription = null) },
                                        onClick = {
                                            menuOpen = false
                                            val clip = ClipData.newPlainText("long capture", state.body)
                                            if (Build.VERSION.SDK_INT >= 33) {
                                                clip.description.extras = PersistableBundle().apply {
                                                    putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true)
                                                }
                                            }
                                            scope.launch { clipboard.setClipEntry(ClipEntry(clip)) }
                                        },
                                    )
                                }
                                DropdownMenuItem(
                                    text = { Text("下書きを削除…") },
                                    leadingIcon = { MoteIcon(MoteGlyph.Delete, contentDescription = null) },
                                    enabled = !state.busy,
                                    onClick = {
                                        menuOpen = false
                                        confirmDelete = true
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text("端末の記録を破棄…") },
                                    leadingIcon = { MoteIcon(MoteGlyph.Delete, contentDescription = null) },
                                    enabled = !state.busy,
                                    onClick = {
                                        menuOpen = false
                                        confirmForget = true
                                    },
                                )
                            }
                        }
                        Button(
                            onClick = onFinish,
                            enabled = canFinish,
                            shapes = ButtonDefaults.shapes(
                                shape = CircleShape,
                                pressedShape = MaterialTheme.shapes.small,
                            ),
                            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
                        ) { Text("完了", maxLines = 1) }
                    }
                    HorizontalDivider(Modifier.widthIn(max = ThreadMaxWidth).padding(horizontal = 20.dp), color = colors.outlineVariant)
                }
            }
        },
        bottomBar = {
            Surface(color = colors.background) {
                Box(
                    modifier = Modifier.fillMaxWidth().windowInsetsPadding(
                        WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Bottom),
                    ),
                    contentAlignment = Alignment.TopCenter,
                ) {
                    Column(
                        modifier = Modifier.widthIn(max = ThreadMaxWidth).fillMaxWidth()
                            .padding(horizontal = 20.dp, vertical = 10.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        LongCaptureSupport(
                            state = state,
                            connection = connection,
                            onCheck = { requestAppendOrCheck(onCheck) },
                            onConnect = onConnect,
                            onAuthorize = onAuthorize,
                            modifier = Modifier.weight(1f, fill = false).heightIn(max = 148.dp),
                        )
                        MoteComposer(
                            value = state.draft,
                            onValueChange = onDraft,
                            placeholder = "続きを書く…",
                            modifier = Modifier.fillMaxWidth(),
                            readOnly = !canEdit, // Frozen drafts stay selectable.
                            maxLines = 4,
                        ) {
                            MoteSendButton(
                                enabled = canEdit && state.canUseNode && state.draft.isNotBlank(),
                                label = "追記",
                                onClick = { requestAppendOrCheck(onAppend) },
                            )
                        }
                    }
                }
            }
        },
    ) { insets ->
        Box(
            modifier = Modifier.fillMaxSize().padding(insets).consumeWindowInsets(insets),
            contentAlignment = Alignment.TopCenter,
        ) {
            // Lazy selection covers composed slices; the More action copies the exact full body.
            SelectionContainer(Modifier.widthIn(max = ThreadMaxWidth).fillMaxSize()) {
                LazyColumn(
                    state = listState,
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(start = 20.dp, top = 18.dp, end = 20.dp),
                ) {
                    items(slices, key = { it.start }) { slice ->
                        LongCaptureThreadSlice(state.body, slice)
                    }
                    item(key = "tail") { Spacer(Modifier.height(12.dp)) }
                }
            }
        }
    }

    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            text = { Text("下書きを削除しますか？") },
            confirmButton = {
                TextButton(
                    enabled = !state.busy,
                    colors = ButtonDefaults.textButtonColors(contentColor = colors.error),
                    onClick = {
                        confirmDelete = false
                        onDeleteDraft()
                    },
                ) { Text("削除") }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("キャンセル") } },
        )
    }

    if (confirmForget) {
        AlertDialog(
            onDismissRequest = { confirmForget = false },
            text = { Text("端末の記録を破棄しますか？") },
            confirmButton = {
                TextButton(
                    enabled = !state.busy,
                    colors = ButtonDefaults.textButtonColors(contentColor = colors.error),
                    onClick = {
                        confirmForget = false
                        onForget()
                    },
                ) { Text("端末の記録を破棄") }
            },
            dismissButton = { TextButton(onClick = { confirmForget = false }) { Text("キャンセル") } },
        )
    }
}

@Composable
private fun LongCaptureSupport(
    state: LongCaptureUi,
    connection: MoteConnection,
    onCheck: () -> Unit,
    onConnect: () -> Unit,
    onAuthorize: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val needsReview = state.pending && !state.busy
    if (!needsReview && state.canUseNode && state.message == null) return
    Column(
        modifier = modifier.fillMaxWidth().verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        if (needsReview) {
            val operation = if (state.finishing) "完了" else "追記"
            LongCaptureNotice(
                glyph = MoteGlyph.Alert,
                message = "${operation}の結果が未確定です。一致した場合だけ採用し、前回の操作は再送しません。",
                actionLabel = if (state.canUseNode) "照合して採用" else null,
                action = onCheck,
                actionEnabled = !state.busy,
            )
        }
        if (!state.canUseNode) {
            val (description, actionLabel, action) = when (connection) {
                MoteConnection.NeedsApproval -> Triple("Harmion の承認が必要です。", "承認をリクエスト", onAuthorize)
                MoteConnection.Failed, MoteConnection.Unavailable -> Triple(
                    if (state.pending) "照合するには再接続してください。" else "下書きはこの端末で編集できます。",
                    "再接続",
                    onConnect,
                )
                MoteConnection.Loading -> Triple("Harmion に接続しています…", null, null)
                MoteConnection.Approving -> Triple("Harmion で承認してください。", null, null)
                MoteConnection.Ready -> Triple("Harmion に未接続です。", null, null)
            }
            LongCaptureNotice(MoteGlyph.Link, description, actionLabel, action, actionEnabled = !state.busy)
        }
        state.message?.let { LongCaptureStatusText(it, Modifier.padding(horizontal = 8.dp, vertical = 4.dp)) }
    }
}

@Composable
private fun LongCaptureNotice(
    glyph: MoteGlyph,
    message: String,
    actionLabel: String?,
    action: (() -> Unit)?,
    actionEnabled: Boolean,
) {
    val colors = MaterialTheme.colorScheme
    Surface(color = colors.surfaceContainer, shape = MaterialTheme.shapes.medium) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 8.dp, top = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            MoteIcon(glyph, contentDescription = null, modifier = Modifier.size(18.dp), tint = colors.secondary)
            LongCaptureStatusText(message, Modifier.weight(1f))
            if (actionLabel != null && action != null) {
                TextButton(onClick = action, enabled = actionEnabled) {
                    Text(actionLabel, softWrap = true)
                }
            }
        }
    }
}

@Composable
private fun LongCaptureStatusText(message: String, modifier: Modifier = Modifier) {
    Text(
        message,
        modifier = modifier.semantics { liveRegion = LiveRegionMode.Polite },
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        style = MaterialTheme.typography.bodySmall,
    )
}

@Composable
internal fun LongCaptureThreadSlice(body: String, slice: ThreadSlice) {
    val colors = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth()) {
        if (slice.startsBlock && slice.start > 0) {
            HorizontalDivider(Modifier.padding(top = 11.dp, bottom = 13.dp), color = colors.outlineVariant)
        }
        Text(
            text = remember(body, slice) { body.substring(slice.start, slice.end) },
            color = colors.onBackground,
            style = MaterialTheme.typography.bodyLarge,
        )
    }
}

internal data class ThreadSlice(val start: Int, val end: Int, val startsBlock: Boolean)

/** Blank lines are visual breaks, not authoritative append boundaries. Bound even huge single paragraphs. */
internal fun threadSlices(body: String): List<ThreadSlice> = buildList {
    var start = 0
    var startsBlock = true
    while (start < body.length) {
        var limit = minOf(start + 1200, body.length)
        var blockBreak = -1
        var whitespaceBreak = -1
        var lineBreaks = 0
        for (index in start until limit) {
            val char = body[index]
            if (char == '\n') {
                // Coalesce very short paragraphs so newline-heavy bodies don't create thousands of tiny rows.
                if (index - start >= 24 && body.getOrNull(index + 1) == '\n') {
                    blockBreak = index
                    break
                }
                if (++lineBreaks == 18) {
                    limit = index + 1
                    break
                }
            }
            if (char.isWhitespace()) whitespaceBreak = index + 1
        }
        var end = when {
            blockBreak >= 0 -> blockBreak
            limit == body.length || body[limit - 1] == '\n' -> limit
            whitespaceBreak > start + (limit - start) * 3 / 4 -> whitespaceBreak
            else -> limit
        }
        if (end < body.length && body[end].isLowSurrogate() && body[end - 1].isHighSurrogate()) end--
        val next = when {
            blockBreak >= 0 -> end + 2
            else -> end
        }
        // An ordinary newline at a slice boundary already has the row break to represent it.
        val displayEnd = if (next < body.length && body[end - 1] == '\n') end - 1 else end
        add(ThreadSlice(start, displayEnd, startsBlock))
        start = next
        startsBlock = blockBreak >= 0
    }
}

private val ThreadMaxWidth = 680.dp
