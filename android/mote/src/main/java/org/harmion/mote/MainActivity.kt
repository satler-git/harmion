package org.harmion.mote

import android.content.ClipDescription
import android.content.Intent
import android.graphics.Color
import android.os.BadParcelableException
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle

class MainActivity : ComponentActivity() {
    private val mote by lazy { ViewModelProvider(this)[MoteViewModel::class.java] }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val initialSharedReturn = mote.state.value.sharedReturn
        // On recreation the original send was already delivered. The retained VM or its
        // journal owns the draft, including edits; the launch intent isn't a backup copy.
        receiveShare(intent, isNewDelivery = savedInstanceState == null)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
        )
        setContent {
            MoteTheme {
                val state by mote.state.collectAsStateWithLifecycle()
                val screenState = rememberSaveableStateHolder()
                var settingsVisible by rememberSaveable { mutableStateOf(false) }
                var preferences by remember { mutableStateOf(MotePreferences.read(this)) }
                var handledSharedReturn by remember { mutableLongStateOf(initialSharedReturn) }
                val sharedReturn = state.sharedReturn
                LaunchedEffect(sharedReturn) {
                    if (sharedReturn != handledSharedReturn) {
                        handledSharedReturn = sharedReturn
                        // A newer delivery may already be visible by the time this effect runs.
                        if (!mote.state.value.sharedVisible) moveTaskToBack(true)
                    }
                }

                if (state.sharedVisible) {
                    SharedCaptureScreen(
                        state = state,
                        onDraft = mote::setSharedDraft,
                        onSend = mote::sendShared,
                        onCancel = mote::cancelShared,
                        onReleasePending = mote::releaseSharedPending,
                        onConnect = mote::reconnect,
                        onAuthorize = mote::retryAuthorization,
                    )
                } else if (sharedReturn != handledSharedReturn) {
                    // Don't briefly reveal the inbox while returning to the sharing app.
                    Surface(Modifier.fillMaxSize()) {}
                } else if (settingsVisible) {
                    MoteSettingsScreen(
                        state = state,
                        preferences = preferences,
                        onBack = { settingsVisible = false },
                        onPreferences = {
                            it.write(this)
                            preferences = it
                        },
                        onConnect = mote::reconnect,
                        onAuthorize = mote::retryAuthorization,
                        onSync = mote::requestSync,
                        onOpenShared = mote::openShared,
                    )
                } else {
                    screenState.SaveableStateProvider("home") {
                        MoteScreen(
                            state = state,
                            onDraft = mote::setDraft,
                            onSubmit = mote::submit,
                            onConnect = mote::reconnect,
                            onAuthorize = mote::retryAuthorization,
                            onArchive = mote::archive,
                            onPin = mote::pin,
                            onDelete = mote::delete,
                            archiveNotice = state.archiveNotice,
                            onUndoArchive = mote::undoArchive,
                            onDismissArchive = mote::dismissArchive,
                            onReleasePending = mote::releasePending,
                            onOpenLong = mote::openLong,
                            onLeaveLong = mote::leaveLong,
                            onLongDraft = mote::setLongDraft,
                            onAppendLong = mote::appendLong,
                            onFinishLong = mote::finishLong,
                            onCheckLong = mote::checkLong,
                            onForgetLong = mote::forgetLong,
                            onDeleteLongDraft = mote::deleteLongDraft,
                            onReply = mote::startReply,
                            onCancelReply = mote::cancelReply,
                            onPushToLong = mote::pushToLong,
                            onBeginEdit = mote::beginEdit,
                            onCancelEdit = mote::cancelEdit,
                            onSync = mote::requestSync,
                            preferences = preferences,
                            onOpenSettings = { settingsVisible = true },
                        )
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        receiveShare(intent, isNewDelivery = true)
    }

    private fun receiveShare(incoming: Intent?, isNewDelivery: Boolean) {
        if (incoming == null || incoming.action != Intent.ACTION_SEND) return
        // Don't keep the incoming payload (or any URI grants) as our current intent.
        setIntent(Intent(this, MainActivity::class.java).setAction(Intent.ACTION_MAIN))
        if (isNewDelivery) incoming.sharedPlainText()?.let(mote::acceptShare)
    }
}

private fun Intent.sharedPlainText(): String? {
    if (action != Intent.ACTION_SEND || type != ClipDescription.MIMETYPE_TEXT_PLAIN) return null
    val extra = try {
        getCharSequenceExtra(Intent.EXTRA_TEXT)
    } catch (_: BadParcelableException) {
        null
    }
    extra?.toString()?.takeIf { it.isNotBlank() }?.let { return it }
    val clip = clipData ?: return null
    if (clip.itemCount != 1 || !clip.description.hasMimeType(ClipDescription.MIMETYPE_TEXT_PLAIN)) return null
    val item = clip.getItemAt(0)
    if (item.uri != null || item.intent != null || item.htmlText != null) return null
    // Read only literal text; coerceToText can resolve content URIs and open files.
    return item.text?.toString()?.takeIf { it.isNotBlank() }
}
