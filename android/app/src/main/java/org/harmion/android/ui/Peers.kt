package org.harmion.android.ui

import android.content.Intent
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Group
import androidx.compose.material.icons.rounded.PersonAdd
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.harmion.android.core.NodeRuntimeController
import org.json.JSONObject

@Composable
fun PeersScreen(controller: NodeRuntimeController) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var invite by remember { mutableStateOf<String?>(null) }
    var members by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }

    fun reload() {
        loading = true
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val admin = controller.adminClient()
                    try {
                        val membersJson = admin.memberView().optJSONArray("members")
                        val list = mutableListOf<JSONObject>()
                        if (membersJson != null) {
                            for (index in 0 until membersJson.length()) {
                                list += membersJson.getJSONObject(index)
                            }
                        }
                        val peerFile = File(controller.nodeRoot, "peer.json")
                        val code =
                            if (peerFile.isFile) PeerCode.encode(peerFile.readBytes()) else null
                        list to code
                    } finally {
                        admin.close()
                    }
                }
            }.onSuccess { (list, code) ->
                members = list
                invite = code
                error = null
            }.onFailure { error = it.message }
            loading = false
        }
    }

    LaunchedEffect(Unit) { reload() }

    LazyColumn(
        modifier = Modifier.fillMaxWidth(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item {
            HarmionCard {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        SectionHeader("招待")
                    }
                    IconButton(onClick = { reload() }) {
                        Icon(Icons.Rounded.Refresh, contentDescription = "再読み込み")
                    }
                }
                when {
                    loading -> CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp)
                    invite != null -> {
                        val bitmap = remember(invite) { QrCode.render(invite!!, 720) }
                        Surface(
                            shape = MaterialTheme.shapes.medium,
                            color = androidx.compose.ui.graphics.Color.White,
                        ) {
                            Image(
                                bitmap = bitmap.asImageBitmap(),
                                contentDescription = "招待コード",
                                modifier = Modifier.fillMaxWidth().padding(12.dp),
                            )
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            FilledTonalButton(
                                onClick = {
                                    val text = invite ?: return@FilledTonalButton
                                    val send =
                                        Intent(Intent.ACTION_SEND)
                                            .setType("text/plain")
                                            .putExtra(Intent.EXTRA_TEXT, text)
                                    context.startActivity(
                                        Intent.createChooser(send, "招待コードを共有"),
                                    )
                                },
                                modifier = Modifier.weight(1f),
                            ) {
                                Icon(
                                    Icons.Rounded.Share,
                                    contentDescription = null,
                                    modifier = Modifier.size(18.dp),
                                )
                                Text("共有", modifier = Modifier.padding(start = 8.dp))
                            }
                            OutlinedButton(onClick = { reload() }, modifier = Modifier.weight(1f)) {
                                Text("更新")
                            }
                        }
                    }
                    else -> {
                        Text(
                            "招待コードを取得できませんでした。ノードが稼働しているか確認してください。",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Button(onClick = { reload() }) { Text("再試行") }
                    }
                }
                error?.let {
                    Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
            }
        }

        item { SectionHeader("メンバー", "${members.size} 台のノード") }

        if (members.isEmpty()) {
            item {
                EmptyState(
                    icon = Icons.Rounded.PersonAdd,
                    title = "メンバーがいません",
                    body = "招待コードを読み取ると参加できます。",
                )
            }
        }

        items(members, key = { it.optString("node_id") }) { member ->
            HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Surface(
                        shape = MaterialTheme.shapes.medium,
                        color = MaterialTheme.colorScheme.secondaryContainer,
                    ) {
                        Icon(
                            Icons.Rounded.Group,
                            contentDescription = null,
                            modifier = Modifier.padding(10.dp).size(20.dp),
                        )
                    }
                    Column(Modifier.weight(1f)) {
                        Text(
                            shortId(member.optString("node_id")),
                            style = MaterialTheme.typography.titleSmall,
                        )

                    }
                    StatusPill(memberStateLabel(member.optString("state")))
                }
            }
        }

        item { androidx.compose.foundation.layout.Spacer(Modifier.size(8.dp)) }
    }
}
