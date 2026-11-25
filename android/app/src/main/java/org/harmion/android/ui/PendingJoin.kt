package org.harmion.android.ui

import android.content.Context
import android.content.Intent
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.HourglassTop
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material.icons.rounded.Sync
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.launch
import org.harmion.android.core.NodeRuntimeController

/** Join request code kept until the existing node approves this device. */
object PendingJoin {
    private const val PREFS = "org.harmion.pending_join"
    private const val KEY = "code"

    fun code(context: Context): String? =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString(KEY, null)
            ?.takeIf { it.isNotBlank() }

    fun save(context: Context, code: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY, code)
            .apply()
    }

    fun clear(context: Context) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .remove(KEY)
            .apply()
    }
}

/**
 * Shown on Home and Peers after joining another cluster. The QR is the same
 * registration the onboarding flow displayed, so it survives process restarts
 * until the node becomes an active member.
 */
@Composable
fun PendingJoinCard(controller: NodeRuntimeController) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snapshot by controller.snapshot.collectAsStateWithLifecycle()
    val active = snapshot?.membership == "active"
    var code by remember { mutableStateOf(PendingJoin.code(context)) }
    var syncing by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(active) {
        if (active) {
            PendingJoin.clear(context)
            code = null
        }
    }
    val current = code ?: return

    HarmionCard(containerColor = MaterialTheme.colorScheme.tertiaryContainer) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Icon(Icons.Rounded.HourglassTop, contentDescription = null)
            Column(Modifier.weight(1f)) {
                Text("参加承認待ち", style = MaterialTheme.typography.titleSmall)
                Text(
                    "既存ノードで読み取って承認",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            TextButton(
                onClick = {
                    PendingJoin.clear(context)
                    code = null
                },
            ) { Text("非表示") }
        }
        val bitmap = remember(current) { QrCode.render(current, 640) }
        Surface(
            shape = MaterialTheme.shapes.medium,
            color = Color.White,
        ) {
            Image(
                bitmap = bitmap.asImageBitmap(),
                contentDescription = "参加要求コード",
                modifier = Modifier.fillMaxWidth().padding(10.dp),
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            OutlinedButton(
                onClick = {
                    val send =
                        Intent(Intent.ACTION_SEND)
                            .setType("text/plain")
                            .putExtra(Intent.EXTRA_TEXT, current)
                    context.startActivity(Intent.createChooser(send, "参加コードを共有"))
                },
                modifier = Modifier.weight(1f),
            ) {
                Icon(Icons.Rounded.Share, contentDescription = null, modifier = Modifier.size(16.dp))
                Text("共有", modifier = Modifier.padding(start = 6.dp))
            }
            Button(
                onClick = {
                    syncing = true
                    message = null
                    scope.launch {
                        runCatching { controller.syncNow() }
                            .onSuccess { message = "同期しました" }
                            .onFailure { message = it.message ?: "同期できませんでした" }
                        syncing = false
                    }
                },
                enabled = !syncing,
                modifier = Modifier.weight(1f),
            ) {
                Icon(Icons.Rounded.Sync, contentDescription = null, modifier = Modifier.size(16.dp))
                Text(if (syncing) "同期中…" else "承認を確認", modifier = Modifier.padding(start = 6.dp))
            }
        }
        message?.let {
            Text(it, style = MaterialTheme.typography.bodySmall)
        }
    }
}
