package org.harmion.mote

import android.app.Application
import android.os.SystemClock
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import java.io.IOException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.retryWhen
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import org.harmion.sdk.AuthorizationRequest
import org.harmion.sdk.Filter
import org.harmion.sdk.Harmion
import org.harmion.sdk.HarmionException
import org.harmion.sdk.HarmionSession
import org.harmion.sdk.NodeStatus
import org.harmion.sdk.Requests

internal enum class MoteConnection { Loading, NeedsApproval, Approving, Ready, Unavailable, Failed }

internal data class MoteUiState(
    val connection: MoteConnection = MoteConnection.Loading,
    val captures: List<Capture> = emptyList(),
    val draft: String = "",
    val pending: Boolean = false,
    val busy: Boolean = false,
    val message: String? = null,
    val draftAvailable: Boolean = false,
    val archiveNotice: ArchiveNotice? = null,
    val archivePreviews: Map<String, ArchivePreview> = emptyMap(),
    val longCapture: LongCaptureUi? = null,
    val longCaptureVisible: Boolean = false,
    val longCaptureAvailable: Boolean = false,
    val longCaptureStorageFailed: Boolean = false,
    val sharedDraft: String = "",
    val sharedPending: Boolean = false,
    val sharedAvailable: Boolean = false,
    val sharedVisible: Boolean = false,
    val sharedMessage: String? = null,
    val sharedReturn: Long = 0L,
    /** True once a complete snapshot has been shown; cached captures stay visible on reconnect. */
    val snapshotLoaded: Boolean = false,
    /** A persisted read-only snapshot was restored before the node answered. */
    val cacheLoaded: Boolean = false,
    /** Target of the short-capture reply currently being composed. */
    val replyTo: String? = null,
    /** Short capture being edited in the home composer. */
    val editingId: String? = null,
    val sync: SyncUi = SyncUi(),
    val syncRequestBusy: Boolean = false,
) {
    override fun toString(): String = "MoteUiState(connection=$connection)"
}

internal data class SyncUi(
    val enabled: Boolean = true,
    val permitted: Boolean = true,
    val blockedCode: String? = null,
    val blockedMessage: String? = null,
    val lastSuccessAtMicros: Long? = null,
    val lastError: String? = null,
    val inFlight: Boolean = false,
    val retrying: Int = 0,
)

internal data class ArchiveNotice(val capture: Capture, val archived: Boolean, val sequence: Long)
internal data class ArchivePreview(val archived: Boolean, val sequence: Long, val archivedAtMicros: Long? = null)

internal data class LongCaptureUi(
    val captureId: String,
    val body: String = "",
    val draft: String = "",
    val pending: Boolean = false,
    val finishing: Boolean = false,
    val canUseNode: Boolean = false,
    val canFinishLocally: Boolean = false,
    val busy: Boolean = false,
    val message: String? = null,
) {
    override fun toString(): String = "LongCaptureUi(redacted)"
}

internal class MoteViewModel(application: Application) : AndroidViewModel(application) {
    private val journal = CaptureJournal(application)
    private val shareJournal = CaptureJournal(application, shared = true)
    private val shareLoaded = CompletableDeferred<Unit>()
    private val shareUse = Mutex()
    private var shareWriter: Job? = null
    private var sharedOnDisk = CaptureDraft()
    private var returnFromShare = false
    private val longJournal = LongCaptureJournal(application)
    private val captureCache = CaptureCache(application)
    private var longRecord: LongCaptureRecord? = null
    private var longSaveJob: Job? = null
    private var longDraftWriter: Job? = null
    private val sessionUse = Mutex()
    private val cacheUse = Mutex()
    private val _state = MutableStateFlow(MoteUiState())
    val state: StateFlow<MoteUiState> = _state
    private var connectionJob: Job? = null
    private var connectionGeneration = 0L
    private var draftWriter: Job? = null
    private var repository: CaptureRepository? = null
    private var draftOnDisk: CaptureDraft? = null
    private var syncPollJob: Job? = null
    private var cacheWriter: Job? = null
    private var frozen: PendingCapture? = null
    private var editBackupDraft: String? = null
    private var editBackupReplyTo: String? = null
    private var editCapture: Capture? = null
    private var noticeSequence = 0L

    init {
        viewModelScope.launch {
            // Restore the last snapshot first so a cold start does not flash the connection screen.
            try {
                val cached = withContext(Dispatchers.IO) { captureCache.load() }
                if (cached != null) {
                    change { copy(captures = cached.items, cacheLoaded = true) }
                }
            } catch (_: IOException) {
                // A corrupt or unreadable cache is best effort; the node snapshot still wins.
            }
            try {
                val draft = withContext(Dispatchers.IO) { journal.load() }
                frozen = draft.pending
                draftOnDisk = draft
                change {
                    copy(
                        draft = draft.body,
                        pending = draft.pending != null,
                        draftAvailable = true,
                        replyTo = draft.replyTo,
                    )
                }
            } catch (_: IOException) {
                // Do not overwrite a journal we could not recover.
                change { copy(message = "端末の下書きを読み込めません。記録を上書きしないため入力を停止しました。") }
            }
            try {
                longRecord = withContext(Dispatchers.IO) { longJournal.load() }
                change { copy(longCaptureAvailable = true, longCapture = longRecord?.toUi()) }
            } catch (_: IOException) {
                change { copy(longCaptureStorageFailed = true) }
            }
            try {
                sharedOnDisk = withContext(Dispatchers.IO) { shareJournal.load() }
                change { copy(sharedAvailable = true, sharedDraft = sharedOnDisk.body, sharedPending = sharedOnDisk.pending != null) }
            } catch (_: IOException) {
                change { copy(sharedMessage = "前の共有メモを読み込めません。上書きしないよう保存を停止しています。") }
            } finally {
                shareLoaded.complete(Unit)
            }
            connect()
        }
    }

    fun connect() = startConnection(approve = false)
    fun authorize() = startConnection(approve = true)
    fun reconnect() = startConnection(approve = false, force = true)
    fun retryAuthorization() = startConnection(approve = true, force = true)

    private fun startConnection(approve: Boolean, force: Boolean = false) {
        val current = _state.value
        if (!force && (current.busy || (approve && current.connection != MoteConnection.NeedsApproval))) return
        if (force && current.busy) change { copy(busy = false) }
        val previous = connectionJob
        val generation = ++connectionGeneration
        if (_state.value.draftAvailable) change { copy(message = null) }
        updateConnection(if (approve) MoteConnection.Approving else MoteConnection.Loading)
        connectionJob = viewModelScope.launch {
            // A cancelled native long-poll may not return promptly. Never let it block a fresh
            // connection; the generation guard below keeps late results from publishing.
            withTimeoutOrNull(1_500L) { previous?.cancelAndJoin() }
            if (generation != connectionGeneration) return@launch
            val self = coroutineContext[Job]
            var opened: HarmionSession? = null
            var bound: CaptureRepository? = null
            var watchdog: Job? = null
            try {
                val session = withTimeout(8_000L) { Harmion.connect(getApplication()).also { opened = it } }
                if (approve) {
                    val scope = Filter.Kinds(listOf(CAPTURE_KIND))
                    session.authorize(AuthorizationRequest(read = scope, write = scope), "Mote")
                    updateConnection(MoteConnection.Loading)
                }
                val captures = CaptureRepository(session, session.authorizationIdentity())
                bound = captures
                if (generation != connectionGeneration) return@launch
                repository = captures
                startSyncPolling(captures)
                watchdog = launch {
                    delay(20_000L)
                    if (generation == connectionGeneration && _state.value.connection != MoteConnection.Ready) {
                        updateConnection(MoteConnection.Unavailable, "Harmion から一覧を取得できませんでした。")
                        self?.cancel()
                    }
                }
                captures.observe().retryWhen { failure, attempt ->
                    val retryable = (failure as? HarmionException)?.code in setOf("transport", "timeout", "reauthorize")
                    if (generation == connectionGeneration && retryable && attempt < 2L) {
                        // Reopen only the read subscription. Outstanding saves are never replayed here.
                        updateConnection(MoteConnection.Loading)
                        delay(650)
                        true
                    } else false
                }.collect { snapshot ->
                    if (generation != connectionGeneration) return@collect
                    when (snapshot) {
                        CaptureSnapshot.Loading -> updateConnection(MoteConnection.Loading)
                        is CaptureSnapshot.Ready -> {
                            watchdog?.cancel()
                            watchdog = null
                            change {
                                val newFlags = snapshot.items.associate { it.id to it.archived }
                                copy(
                                    connection = MoteConnection.Ready,
                                    captures = snapshot.items,
                                    snapshotLoaded = true,
                                    cacheLoaded = false,
                                    archivePreviews = archivePreviews.filter { (id, preview) ->
                                        id in newFlags && newFlags[id] != preview.archived
                                    },
                                )
                            }
                            scheduleCacheWrite(snapshot.items)
                        }
                    }
                }
                if (generation == connectionGeneration) {
                    updateConnection(MoteConnection.Failed, "Harmion との接続が終了しました。")
                }
            } catch (_: TimeoutCancellationException) {
                if (generation == connectionGeneration) {
                    updateConnection(MoteConnection.Unavailable, "Harmion に接続できませんでした。")
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                if (generation == connectionGeneration) {
                    val (connection, reason) = connectionFailure(error)
                    updateConnection(connection, reason)
                }
            } catch (_: LinkageError) {
                if (generation == connectionGeneration) {
                    updateConnection(MoteConnection.Failed, "この端末で Harmion SDK を起動できませんでした。")
                }
            } finally {
                watchdog?.cancel()
                if (bound != null && repository === bound) repository = null
                if (generation == connectionGeneration) {
                    syncPollJob?.cancel()
                    syncPollJob = null
                }
                opened?.let { session ->
                    withContext(NonCancellable) {
                        sessionUse.withLock { withContext(Dispatchers.IO) { session.close() } }
                    }
                }
            }
        }
    }

    fun setDraft(body: String) {
        if (!_state.value.draftAvailable || frozen != null || _state.value.busy) return
        if (body.length > MAX_DRAFT_CHARS) {
            change { copy(message = "short capture が長すぎます。") }
            return
        }
        change { copy(draft = body, message = null) }
        if (_state.value.editingId == null) scheduleDraftWrite()
    }

    /** Starts (or changes) the reply target of the short-capture composer. */
    fun startReply(capture: Capture) {
        val state = _state.value
        if (!state.draftAvailable || state.pending || state.busy || state.editingId != null) return
        if (state.captures.none { it.id == capture.id }) return
        change { copy(replyTo = capture.id, message = null) }
        scheduleDraftWrite()
    }

    fun cancelReply() {
        if (_state.value.replyTo == null || frozen != null) return
        change { copy(replyTo = null) }
        scheduleDraftWrite()
    }
    fun beginEdit(capture: Capture) {
        val state = _state.value
        if (state.editingId != null || capture.isLong || !state.draftAvailable || state.pending || state.busy ||
            state.connection != MoteConnection.Ready || state.captures.none { it.id == capture.id }
        ) return
        editBackupDraft = state.draft
        editBackupReplyTo = state.replyTo
        editCapture = capture
        change { copy(editingId = capture.id, draft = capture.body, replyTo = null, message = null) }
    }

    fun cancelEdit() {
        val state = _state.value
        if (state.editingId == null || state.busy) return
        val backupDraft = editBackupDraft ?: ""
        val backupReplyTo = editBackupReplyTo
        editBackupDraft = null
        editBackupReplyTo = null
        editCapture = null
        change { copy(editingId = null, draft = backupDraft, replyTo = backupReplyTo, message = null) }
        scheduleDraftWrite()
    }

    private fun submitEdit() {
        val state = _state.value
        val original = editCapture ?: return
        if (state.editingId == null) return
        val body = state.draft
        if (body.isBlank() || body == original.body) return
        val backupDraft = editBackupDraft ?: ""
        val backupReplyTo = editBackupReplyTo
        mutate(
            original.id,
            onFailure = { error -> change { copy(message = mutationFailure(error, false)) } },
        ) { captures ->
            captures.editBody(original, body)
            draftWriter?.join()
            editBackupDraft = null
            editBackupReplyTo = null
            editCapture = null
            change { copy(editingId = null, draft = backupDraft, replyTo = backupReplyTo, message = null) }
            runCatching { persist(CaptureDraft(backupDraft, replyTo = backupReplyTo)) }
        }
    }


    private fun scheduleDraftWrite() {
        if (draftWriter?.isActive == true) return
        draftWriter = viewModelScope.launch {
            while (!_state.value.busy && frozen == null) {
                if (_state.value.editingId != null) break
                val current = _state.value
                val latest = CaptureDraft(current.draft, replyTo = current.replyTo)
                if (latest == draftOnDisk) break
                try {
                    persist(latest)
                } catch (_: IOException) {
                    change { copy(message = "端末に下書きを保存できません。アプリを閉じる前に本文を退避してください。") }
                    break
                }
            }
        }
    }

    fun submit() {
        val state = _state.value
        if (!state.draftAvailable) return
        if (state.editingId != null) {
            submitEdit()
            return
        }
        val body = state.draft
        if (body.isBlank()) return
        val replyTo = state.replyTo
        mutate(create = true) { captures ->
            draftWriter?.join()
            val pending = frozen ?: captures.prepareCreate(body, replyTo).also {
                frozen = it
                change { copy(pending = true) }
            }
            // Do not cross the network boundary until the exact request is durable. A failed
            // journal write also keeps the same in-memory request for the next explicit retry.
            persist(CaptureDraft(body, pending, replyTo))
            captures.create(pending)
            try {
                persist(CaptureDraft())
            } catch (_: IOException) {
                change { copy(message = "Harmion に保存済みですが、端末の保留記録を消せませんでした。") }
                return@mutate
            }
            frozen = null
            change { copy(draft = "", pending = false, replyTo = null, message = "Harmion に保存しました。") }
        }
    }

    fun releasePending() {
        if (frozen == null || !_state.value.draftAvailable || _state.value.busy) return
        change { copy(busy = true, message = null) }
        viewModelScope.launch {
            try {
                draftWriter?.join()
                val state = _state.value
                persist(CaptureDraft(state.draft, replyTo = state.replyTo))
                frozen = null
                change { copy(pending = false, message = "保留を解除しました。本文は下書きに残っています。") }
            } catch (_: IOException) {
                change { copy(message = "保留を解除できませんでした。") }
            } finally {
                change { copy(busy = false) }
            }
        }
    }

    fun acceptShare(body: String) {
        if (body.isBlank() || body.length > MAX_DRAFT_CHARS) return
        returnFromShare = true
        change { copy(sharedVisible = true) }
        viewModelScope.launch {
            shareLoaded.await()
            shareUse.withLock {
                if (!_state.value.sharedVisible || !_state.value.sharedAvailable) return@withLock
                if (_state.value.sharedDraft.isNotBlank() || _state.value.sharedPending) {
                    if (_state.value.sharedDraft != body) {
                        change { copy(sharedMessage = "前の共有メモを表示しています。今回の共有内容で上書きしていません。") }
                    }
                    return@withLock
                }
                change { copy(sharedDraft = body, sharedMessage = null) }
                try {
                    persistShared(CaptureDraft(body))
                } catch (_: IOException) {
                    change { copy(sharedMessage = "端末に保存できませんでした。本文は画面に残しています。") }
                }
            }
        }
    }

    fun openShared() {
        if (_state.value.busy || _state.value.sharedDraft.isBlank()) return
        returnFromShare = false
        change { copy(sharedVisible = true) }
    }

    fun setSharedDraft(body: String) {
        val state = _state.value
        if (!state.sharedVisible || !state.sharedAvailable || state.sharedPending || state.busy || body.length > MAX_DRAFT_CHARS) return
        change { copy(sharedDraft = body, sharedMessage = null) }
        if (shareWriter?.isActive == true) return
        shareWriter = viewModelScope.launch {
            shareUse.withLock {
                while (!_state.value.busy && !_state.value.sharedPending) {
                    val latest = _state.value.sharedDraft
                    if (latest == sharedOnDisk.body) break
                    try {
                        persistShared(CaptureDraft(latest))
                    } catch (_: IOException) {
                        change { copy(sharedMessage = "端末に下書きを保存できません。本文をコピーして退避できます。") }
                        break
                    }
                }
            }
        }
    }

    fun sendShared() {
        val state = _state.value
        if (!state.sharedVisible || !state.sharedAvailable || state.sharedDraft.isBlank()) return
        mutate(onFailure = { error -> change { copy(sharedMessage = mutationFailure(error, true)) } }) { captures ->
            shareUse.withLock {
                val body = _state.value.sharedDraft
                val pending = sharedOnDisk.pending ?: captures.prepareCreate(body)
                persistShared(CaptureDraft(body, pending))
                change { copy(sharedPending = true, sharedMessage = null) }
                captures.create(pending)
                try {
                    persistShared(CaptureDraft())
                } catch (_: IOException) {
                    change { copy(sharedMessage = "Harmion に保存しましたが、端末の保留記録を消去できませんでした。") }
                    return@withLock
                }
                closeShared(clear = true)
            }
        }
    }

    fun releaseSharedPending() {
        val current = _state.value
        if (!current.sharedVisible || !current.sharedAvailable || !current.sharedPending || current.busy) return
        change { copy(busy = true) }
        viewModelScope.launch {
            try {
                shareUse.withLock {
                    persistShared(CaptureDraft(_state.value.sharedDraft))
                    change { copy(sharedPending = false, sharedMessage = null) }
                }
            } catch (_: IOException) {
                change { copy(sharedMessage = "保留を解除できませんでした。") }
            } finally {
                change { copy(busy = false) }
            }
        }
    }

    fun cancelShared() {
        if (!_state.value.sharedVisible || _state.value.busy) return
        change { copy(busy = true) }
        viewModelScope.launch {
            try {
                shareUse.withLock {
                    val canClear = _state.value.sharedAvailable && !_state.value.sharedPending
                    if (canClear) persistShared(CaptureDraft())
                    closeShared(clear = canClear)
                }
            } catch (_: IOException) {
                change { copy(sharedMessage = "端末の共有下書きを消去できませんでした。") }
            } finally {
                change { copy(busy = false) }
            }
        }
    }

    private fun closeShared(clear: Boolean) {
        val returnToSender = returnFromShare
        returnFromShare = false
        change {
            copy(
                sharedVisible = false,
                sharedDraft = if (clear) "" else sharedDraft,
                sharedPending = if (clear) false else sharedPending,
                sharedMessage = null,
                sharedReturn = sharedReturn + if (returnToSender) 1 else 0,
            )
        }
    }

    private suspend fun persistShared(record: CaptureDraft) {
        withContext(Dispatchers.IO) { shareJournal.write(record) }
        sharedOnDisk = record
    }

    fun openLong() {
        if (!_state.value.longCaptureAvailable || _state.value.busy) return
        if (longRecord != null) {
            change { copy(longCaptureVisible = true, longCapture = longCapture?.copy(message = null)) }
            return
        }
        if (_state.value.pending) return
        val movedDraft = _state.value.draft.takeIf { it.isNotBlank() }
        localLongOperation {
            draftWriter?.join()
            persistLong(LongCaptureRecord(Requests.randomIdHex(), draft = movedDraft.orEmpty()))
            if (movedDraft != null) {
                // Continue the short draft in the long capture instead of leaving a duplicate.
                change { copy(draft = "", replyTo = null) }
                runCatching { persist(CaptureDraft("", replyTo = null)) }
            }
            change { copy(longCaptureVisible = true, longCapture = longCapture?.copy(message = null)) }
        }
    }

    fun leaveLong() = change {
        copy(longCaptureVisible = false, longCapture = longCapture?.copy(message = null))
    }

    fun setLongDraft(body: String) {
        val current = longRecord ?: return
        if (!_state.value.longCaptureVisible || _state.value.busy || current.pending != null) return
        if (body.length > MAX_DRAFT_CHARS) {
            longMessage("追記の下書きが長すぎます。")
            return
        }
        change { copy(longCapture = longCapture?.copy(draft = body, message = null)) }
        if (longDraftWriter?.isActive == true) return
        longDraftWriter = viewModelScope.launch {
            while (!_state.value.busy) {
                val record = longRecord ?: break
                val latest = _state.value.longCapture?.draft ?: break
                if (record.pending != null || record.draft == latest) break
                try {
                    persistLong(record.copy(draft = latest), publish = false)
                } catch (_: IOException) {
                    longMessage("端末に下書きを保存できません。アプリを閉じる前に本文を退避してください。")
                    break
                }
            }
        }
    }

    fun appendLong() {
        if (_state.value.longCapture?.draft?.isNotBlank() != true || longRecord?.pending != null) return
        runLong { record -> append(record) }
    }

    fun finishLong() {
        val draft = _state.value.longCapture?.draft ?: return
        val current = longRecord ?: return
        if (current.pending != null || !_state.value.longCaptureVisible || _state.value.busy) return
        if (current.identity == null && draft.isBlank()) {
            localLongOperation {
                longDraftWriter?.join()
                persistLong(null)
                change { copy(message = "long capture を閉じました。") }
            }
            return
        }
        runLong { record ->
            if (record.draft.isNotBlank()) append(record)
            finish(checkNotNull(longRecord).copy(draft = ""))
            change { copy(message = "long capture を保存しました。") }
        }
    }

    fun checkLong() {
        if (longRecord?.pending == null) return
        runLong { record ->
            checkPending(record)
            if (record.pending?.kind == LongCaptureKind.Finish) {
                change { copy(message = "long capture を保存しました。") }
            } else {
                longMessage("Harmion の現在の本文を採用しました。")
            }
        }
    }

    fun forgetLong() {
        if (!_state.value.longCaptureVisible || longRecord == null || _state.value.busy) return
        localLongOperation {
            longDraftWriter?.join()
            persistLong(null)
            change { copy(message = "端末の long capture 記録だけを破棄しました。") }
        }
    }

    /**
     * Deletes the long draft. When the draft has already reached Harmion, the session is
     * closed and the Content is deleted as well; an unsent local draft is discarded.
     */
    fun deleteLongDraft() {
        val record = longRecord ?: return
        val state = _state.value
        if (state.busy || longSaveJob?.isActive == true) return
        if (record.pending != null) {
            longOrHomeMessage("結果が未確定のため削除できません。先に照合してください。")
            return
        }
        if (record.identity == null) {
            localLongOperation {
                longDraftWriter?.join()
                persistLong(null)
                change { copy(longCaptureVisible = false, message = "下書きを削除しました。") }
            }
            return
        }
        if (state.connection != MoteConnection.Ready) {
            longOrHomeMessage("Harmion に接続してから削除してください。")
            return
        }
        mutate(onFailure = { error -> longOrHomeMessage(longFailure(error)) }) { captures ->
            longDraftWriter?.join()
            try {
                captures.deleteDraftSession(record.id)
            } catch (error: HarmionException) {
                // A draft deleted from another device is already in the desired state.
                if (error.code != "http_404" && error.code != "http_410") throw error
            }
            persistLong(null)
            change { copy(longCaptureVisible = false, message = "下書きを削除しました。") }
        }
    }

    private fun longOrHomeMessage(message: String) {
        change { copy(message = message) }
        if (_state.value.longCaptureVisible) longMessage(message)
    }

    /** Appends selected short captures as separate blocks and removes the originals. */
    fun pushToLong(ids: List<String>) {
        val state = _state.value
        if (state.busy) return
        if (longRecord?.pending != null) {
            change { copy(message = "long capture の結果が未確定です。先に照合してください。") }
            return
        }
        if (state.connection != MoteConnection.Ready) {
            change { copy(message = "Harmion に接続してから long capture へ移動してください。") }
            return
        }
        val selected = state.captures
            .filter { it.id in ids && !it.isLong && it.body.isNotBlank() }
            .sortedWith(compareBy(Capture::createdAtMicros, Capture::id))
        if (selected.isEmpty()) return
        mutate(onFailure = { error -> change { copy(message = longFailure(error)) } }) { captures ->
            longDraftWriter?.join()
            val record = longRecord ?: LongCaptureRecord(Requests.randomIdHex())
            val parts = buildList {
                record.draft.trim().takeIf { it.isNotEmpty() }?.let(::add)
                selected.forEach { add(it.body.trim()) }
            }
            LongCaptureSession(captures) { persistLong(it) }.append(
                record.copy(draft = parts.joinToString("\n\n")),
            )
            change { copy(longCaptureVisible = true, longCapture = longRecord?.toUi()) }
            var failed = 0
            for (item in selected) {
                try {
                    captures.delete(item)
                } catch (error: Exception) {
                    if (error is CancellationException) throw error
                    if ((error as? HarmionException)?.code in connectionErrorCodes) throw error
                    failed++
                }
            }
            val summary = if (failed == 0) {
                "${selected.size}件を long capture に移動しました。"
            } else {
                "${selected.size}件を long capture に移動しました。${failed}件は削除できませんでした。"
            }
            longMessage(summary)
        }
    }

    /**
     * Long captures save in the background: the screen is not globally busy and the user can
     * leave it while the frozen write completes. The journal/pending state still serializes the
     * node write and keeps it crash-recoverable.
     */
    private fun runLong(operation: suspend LongCaptureSession.(LongCaptureRecord) -> Unit) {
        if (!_state.value.longCaptureVisible || longRecord == null) return
        if (longSaveJob?.isActive == true) return
        val captures = repository ?: return
        if (_state.value.connection != MoteConnection.Ready) {
            longMessage("Harmion に接続してから保存してください。")
            return
        }
        longSaveJob = viewModelScope.launch {
            try {
                sessionUse.withLock {
                    if (captures !== repository || _state.value.connection != MoteConnection.Ready) return@withLock
                    longDraftWriter?.join()
                    val record = checkNotNull(longRecord)
                    val draft = checkNotNull(_state.value.longCapture).draft
                    longMessage(null)
                    val input = record.copy(draft = draft)
                    if (input != record) persistLong(input, publish = false)
                    LongCaptureSession(captures) { persistLong(it) }.operation(input)
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                when ((error as? HarmionException)?.code) {
                    "not_authorized", "http_401", "oauth_invalid_grant", "node_unavailable" -> {
                        val (connection, reason) = connectionFailure(error)
                        connectionJob?.cancel()
                        updateConnection(connection, reason)
                    }
                    else -> longMessage(longFailure(error))
                }
            } finally {
                longSaveJob = null
            }
        }
    }

    private fun localLongOperation(action: suspend () -> Unit) {
        change { copy(busy = true) }
        viewModelScope.launch {
            try {
                action()
            } catch (_: IOException) {
                if (_state.value.longCaptureVisible) longMessage("端末の long capture 記録を更新できませんでした。")
                else change { copy(message = "端末に long capture セッションを作成できませんでした。") }
            } finally {
                change { copy(busy = false) }
            }
        }
    }

    private suspend fun persistLong(record: LongCaptureRecord?, publish: Boolean = true) {
        withContext(Dispatchers.IO) { longJournal.write(record) }
        longRecord = record
        if (publish) change {
            copy(longCapture = record?.toUi(), longCaptureVisible = record != null && longCaptureVisible)
        }
    }

    private fun LongCaptureRecord.toUi() = LongCaptureUi(
        captureId = id,
        body = body,
        draft = draft,
        pending = pending != null,
        finishing = pending?.kind == LongCaptureKind.Finish,
        canFinishLocally = identity == null,
    )

    private fun longMessage(message: String?) = change {
        copy(longCapture = longCapture?.copy(message = message))
    }

    private fun longFailure(error: Exception): String = when ((error as? HarmionException)?.code) {
        "long_authorization_changed" -> "作成時と認可先が異なるため操作していません。本文をコピーして退避できます。"
        "long_capture_changed", "http_404", "http_410" ->
            "Harmion の現在の本文や状態を確認できません。追記や完了は行わず、端末の本文と下書きを残しています。"
        "capture_too_large", "http_413" -> "本文がサイズ上限を超えています。下書きを短くするか退避してください。"
        "http_403" -> "この操作に必要な Harmion の権限がありません。"
        else -> if (error is IOException) "端末の long capture 記録を更新できません。下書きは画面に残しています。"
        else "Harmion での保存結果を確認できません。下書きは端末に残っています。"
    }

    fun archive(capture: Capture, value: Boolean) = archive(capture, value, offerUndo = true)

    private fun archive(
        capture: Capture,
        value: Boolean,
        offerUndo: Boolean,
        restoreArchiveTime: Boolean = false,
    ) {
        // Undo restores the original position. A legacy capture without a time stays legacy.
        val archiveTime = if (value) {
            if (restoreArchiveTime) capture.archivedAtMicros else System.currentTimeMillis() * 1_000L
        } else {
            null
        }
        val preview = ArchivePreview(
            archived = value,
            sequence = ++noticeSequence,
            archivedAtMicros = archiveTime,
        )
        fun clearPreview() = change {
            if (archivePreviews[capture.id]?.sequence == preview.sequence) {
                copy(archivePreviews = archivePreviews - capture.id)
            } else this
        }
        mutate(
            capture.id,
            onBegin = { change { copy(archivePreviews = archivePreviews + (capture.id to preview)) } },
            onFailure = { error -> clearPreview(); change { copy(message = mutationFailure(error, false)) } },
        ) { captures ->
            captures.setArchived(capture, value, archiveTime)
            if (offerUndo && _state.value.connection == MoteConnection.Ready) {
                change { copy(archiveNotice = ArchiveNotice(capture, value, preview.sequence)) }
            }
            // The subscription normally clears this first. Never mask a later node state indefinitely.
            viewModelScope.launch { delay(8_000); clearPreview() }
        }
    }

    fun undoArchive(notice: ArchiveNotice) {
        if (_state.value.archiveNotice?.sequence != notice.sequence || _state.value.busy) return
        dismissArchive(notice)
        // The subscription can lag; the repository verifies the actual attribute before writing.
        archive(
            notice.capture.copy(archived = notice.archived),
            notice.capture.archived,
            offerUndo = false,
            restoreArchiveTime = true,
        )
    }

    fun dismissArchive(notice: ArchiveNotice) {
        change { if (archiveNotice?.sequence == notice.sequence) copy(archiveNotice = null) else this }
    }

    fun editShort(capture: Capture, body: String) {
        if (capture.isLong || body.isBlank() || body.length > MAX_DRAFT_CHARS || body == capture.body) return
        mutate(
            capture.id,
            onFailure = { error -> change { copy(message = mutationFailure(error, false)) } },
        ) { captures ->
            captures.editBody(capture, body)
        }
    }

    fun pin(capture: Capture, value: Boolean) = mutate(capture.id) { it.setPinned(capture, value) }

    fun delete(capture: Capture) {
        if (capture.sessionOpen && longRecord?.id == capture.id && longRecord?.pending != null) {
            change { copy(message = "結果が未確定のため削除できません。先に照合してください。") }
            return
        }
        mutate(capture.id) { captures ->
            if (capture.sessionOpen) captures.deleteDraftSession(capture) else captures.delete(capture)
            if (longRecord?.id == capture.id) {
                longDraftWriter?.join()
                persistLong(null)
            }
            change { copy(message = "削除しました。") }
        }
    }

    private fun mutate(
        captureId: String? = null,
        create: Boolean = false,
        onBegin: (() -> Unit)? = null,
        onFailure: ((Exception) -> Unit)? = null,
        action: suspend (CaptureRepository) -> Unit,
    ) {
        val captures = repository ?: return
        val state = _state.value
        if (state.connection != MoteConnection.Ready || state.busy ||
            (captureId != null && state.captures.none { it.id == captureId })
        ) return
        change { copy(busy = true, message = null) }
        onBegin?.invoke()
        viewModelScope.launch {
            try {
                sessionUse.withLock {
                    if (captures === repository && _state.value.connection == MoteConnection.Ready) action(captures)
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                when ((error as? HarmionException)?.code) {
                    "not_authorized", "http_401", "oauth_invalid_grant", "node_unavailable" -> {
                        val (connection, reason) = connectionFailure(error)
                        connectionJob?.cancel()
                        updateConnection(connection, reason)
                    }
                    else -> if (onFailure != null) onFailure(error) else change { copy(message = mutationFailure(error, create)) }
                }
            } finally {
                change { copy(busy = false) }
            }
        }
    }

    private suspend fun persist(draft: CaptureDraft) {
        withContext(Dispatchers.IO) { journal.write(draft) }
        draftOnDisk = draft
    }

    private fun change(transform: MoteUiState.() -> MoteUiState) {
        val next = _state.value.transform()
        _state.value = next.copy(longCapture = next.longCapture?.copy(
            canUseNode = next.connection == MoteConnection.Ready,
            busy = next.busy,
        ))
    }

    /** Cached captures intentionally survive reconnect attempts; mutations stay disabled until Ready. */
    private fun updateConnection(connection: MoteConnection, reason: String? = null) = change {
        copy(
            connection = connection,
            archiveNotice = null,
            archivePreviews = emptyMap(),
            message = reason ?: message,
            sync = if (connection == MoteConnection.Ready) sync else sync.copy(inFlight = false, retrying = 0),
            syncRequestBusy = if (connection == MoteConnection.Ready) syncRequestBusy else false,
        )
    }

    private fun scheduleCacheWrite(items: List<Capture>) {
        cacheWriter?.cancel()
        cacheWriter = viewModelScope.launch {
            delay(700L)
            runCatching {
                withContext(Dispatchers.IO) { cacheUse.withLock { captureCache.write(items) } }
            }
        }
    }

    private fun startSyncPolling(captures: CaptureRepository) {
        syncPollJob?.cancel()
        syncPollJob = viewModelScope.launch {
            while (repository === captures) {
                runCatching { captures.syncStatus() }
                    .onSuccess { status -> change { copy(sync = status.toSyncUi()) } }
                delay(5_000L)
            }
        }
    }

    fun requestSync() {
        val captures = repository ?: return
        if (_state.value.connection != MoteConnection.Ready || _state.value.syncRequestBusy) return
        change { copy(syncRequestBusy = true, message = null) }
        viewModelScope.launch {
            try {
                captures.requestSync()
                // Keep the control visibly spinning even when the node coalesces the request.
                delay(900L)
                runCatching { captures.syncStatus() }
                    .onSuccess { status -> change { copy(sync = status.toSyncUi()) } }
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                change { copy(message = "同期を要求できませんでした。") }
            } finally {
                change { copy(syncRequestBusy = false) }
            }
        }
    }

    private fun NodeStatus.toSyncUi() = SyncUi(
        enabled = syncEnabled,
        permitted = syncPermitted,
        blockedCode = syncBlockedCode,
        blockedMessage = syncBlockedMessage,
        lastSuccessAtMicros = lastSuccessAtMicros,
        lastError = lastError,
        inFlight = syncInFlight.isNotEmpty(),
        retrying = syncRetrying,
    )

    private fun connectionFailure(error: Exception): Pair<MoteConnection, String> {
        val code = (error as? HarmionException)?.code
        android.util.Log.w("Mote", "connection failure: ${code ?: error.javaClass.simpleName}")
        return when (code) {
            "not_authorized", "http_401", "oauth_invalid_grant" ->
                MoteConnection.NeedsApproval to "Harmion の承認が必要です。"
            "denied", "approval_denied" -> MoteConnection.NeedsApproval to "接続は承認されませんでした。"
            "approval_timeout" -> MoteConnection.NeedsApproval to "承認の待ち時間が終了しました。"
            "approval_expired" -> MoteConnection.NeedsApproval to "承認要求の有効期限が切れました。"
            "completed" -> MoteConnection.NeedsApproval to "承認は完了しましたが、接続できませんでした。"
            "timeout" -> MoteConnection.Unavailable to "Harmion との通信がタイムアウトしました。"
            "expired" -> MoteConnection.Unavailable to "Harmion との通信の有効期限が切れました。"
            "node_untrusted" -> MoteConnection.Unavailable to "Harmion の署名が一致しません。"
            "node_unavailable" -> MoteConnection.Unavailable to "Harmion が利用できません。インストールと起動状態を確認してください。"
            "capture_reset_limit" -> MoteConnection.Failed to "一覧を取得し直せませんでした。再接続してください。"
            else -> MoteConnection.Failed to "Harmion との接続を確認できません。再接続してください。"
        }
    }

    private fun mutationFailure(error: Exception, create: Boolean): String = when {
        error is IOException -> "端末に保存できないため、Harmion には送信していません。"
        error is HarmionException -> when (error.code) {
            "capture_too_large" -> "short capture が長すぎます。本文を短くしてください。"
            "pending_authorization_changed" -> "送信時と認可先が異なるため再送していません。本文を確認して保留を解除できます。"
            "capture_changed", "http_410", "http_404" -> "capture の最新状態を確認してから、やり直してください。"
            "capture_shared" -> "ほかの種類にも属する capture は Mote から削除できません。"
            "http_403" -> "この操作に必要な Harmion の権限がありません。"
            else -> if (create) UNKNOWN_CREATE else UNKNOWN_MUTATION
        }
        else -> if (create) UNKNOWN_CREATE else UNKNOWN_MUTATION
    }

    private companion object {
        const val MAX_DRAFT_CHARS = 1024 * 1024
        const val UNKNOWN_CREATE = "Harmion での保存を確認できません。本文は端末に残っています。"
        const val UNKNOWN_MUTATION = "操作結果を確認できません。一覧で最新状態を確認してください。"
        val connectionErrorCodes = setOf("not_authorized", "http_401", "oauth_invalid_grant", "node_unavailable")
    }
}
