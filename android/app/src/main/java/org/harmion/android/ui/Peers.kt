package org.harmion.android.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
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
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.journeyapps.barcodescanner.ScanContract
import org.harmion.android.HarmionApplication
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
    var approvalOpen by remember { mutableStateOf(false) }
    var approvalCode by remember { mutableStateOf("") }
    var approvalBusy by remember { mutableStateOf(false) }
    var approvalError by remember { mutableStateOf<String?>(null) }
    var approvalNotice by remember { mutableStateOf<String?>(null) }
    val activity = context as? FragmentActivity
    val application = context.applicationContext as HarmionApplication
    val snapshot by controller.snapshot.collectAsStateWithLifecycle()
    val active = snapshot?.membership == "active"

    val scanLauncher =
        rememberLauncherForActivityResult(ScanContract()) { result ->
            result?.contents?.let { approvalCode = it }
        }
    val cameraPermission =
        rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted) {
                scanLauncher.launch(harmionScanOptions())
            } else {
                approvalError = "カメラ権限が必要です"
            }
        }
    fun scanApproval() {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            scanLauncher.launch(harmionScanOptions())
        } else {
            cameraPermission.launch(Manifest.permission.CAMERA)
        }
    }

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
            }.onFailure { error = errorLabel(it) }
            loading = false
        }
    }

    LaunchedEffect(Unit) { reload() }

    LazyColumn(
        modifier = Modifier.fillMaxWidth(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        // Show one direction at a time: a node that is still waiting for
        // admission cannot invite others, and an active node no longer has a
        // pending request to display.
        if (!active) {
            item { PendingJoinCard(controller) }
        } else {
            item {
                HarmionCard {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            SectionHeader("招待QR")
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
                                "招待コードを取得できませんでした。",
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
        }

        if (active) {
            item {
                OutlinedButton(
                    onClick = {
                        approvalCode = ""
                        approvalError = null
                        approvalNotice = null
                        approvalOpen = true
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Icon(Icons.Rounded.QrCodeScanner, contentDescription = null, modifier = Modifier.size(18.dp))
                    Text("参加要求を承認", modifier = Modifier.padding(start = 8.dp))
                }
                approvalNotice?.let {
                    Text(it, style = MaterialTheme.typography.bodySmall)
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

    if (approvalOpen) {
        val decoded =
            remember(approvalCode) {
                runCatching {
                    PeerCode.decode(approvalCode)?.let { JSONObject(String(it, Charsets.UTF_8)) }
                }.getOrNull()
            }
        AlertDialog(
            onDismissRequest = { if (!approvalBusy) approvalOpen = false },
            title = { Text("参加を承認") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        value = approvalCode,
                        onValueChange = { approvalCode = it.trim() },
                        label = { Text("参加コード") },
                        minLines = 3,
                        maxLines = 6,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    decoded?.let {
                        Text(
                            "クラスタ ${shortId(it.optString("cluster_id"))} / " +
                                "ノード ${shortId(it.optString("node_id"))}",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    approvalError?.let {
                        Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                    }
                    TextButton(onClick = { scanApproval() }, enabled = !approvalBusy) {
                        Icon(Icons.Rounded.QrCodeScanner, contentDescription = null, modifier = Modifier.size(16.dp))
                        Text("QRを読み取る", modifier = Modifier.padding(start = 6.dp))
                    }
                }
            },
            confirmButton = {
                TextButton(
                    enabled = !approvalBusy && decoded != null && activity != null,
                    onClick = {
                        val bytes = PeerCode.decode(approvalCode) ?: return@TextButton
                        approvalBusy = true
                        approvalError = null
                        scope.launch {
                            runCatching {
                                withContext(Dispatchers.IO) {
                                    val admin = controller.adminClient()
                                    try {
                                        application.ownerAdmin.approveNode(admin, activity, bytes)
                                    } finally {
                                        admin.close()
                                    }
                                }
                            }.onSuccess {
                                approvalOpen = false
                                approvalNotice = "承認しました。相手の同期後にメンバーへ反映されます。"
                                reload()
                            }.onFailure {
                                approvalError = errorLabel(it)
                            }
                            approvalBusy = false
                        }
                    },
                ) { Text(if (approvalBusy) "承認中…" else "承認") }
            },
            dismissButton = {
                TextButton(onClick = { if (!approvalBusy) approvalOpen = false }) { Text("キャンセル") }
            },
        )
    }
}
