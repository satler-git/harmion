package org.harmion.android.ui

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Block
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.Group
import androidx.compose.material.icons.rounded.PersonAdd
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material.icons.rounded.WarningAmber
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.SheetValue
import androidx.compose.material3.rememberBottomSheetState
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.journeyapps.barcodescanner.ScanContract
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.harmion.android.HarmionApplication
import org.harmion.android.core.NodeRuntimeController
import org.harmion.android.ui.theme.HarmionSuccess
import org.harmion.sdk.Hex
import org.json.JSONArray
import org.json.JSONObject

private data class PeersData(
    val members: List<JSONObject>,
    val pendingRetirements: List<JSONObject>,
    val syncByNode: Map<String, JSONObject>,
    val syncStatusError: String?,
    val selfNodeId: String,
    val invite: String?,
)

/**
 * One line of sync evidence for a peer. "Reachability" is not a live probe, so
 * the UI shows the recorded last sync instead of inventing an online state.
 */
private data class PeerSyncLine(
    val text: String,
    val isError: Boolean,
)

@Composable
fun PeersScreen(controller: NodeRuntimeController) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val application = context.applicationContext as HarmionApplication
    val activity = context as? FragmentActivity
    var invite by remember { mutableStateOf<String?>(null) }
    var members by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var pendingRetirements by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var syncByNode by remember { mutableStateOf<Map<String, JSONObject>>(emptyMap()) }
    var syncStatusError by remember { mutableStateOf<String?>(null) }
    var selfNodeId by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    var notice by remember { mutableStateOf<String?>(null) }
    var actionError by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var approvalOpen by remember { mutableStateOf(false) }
    var approvalCode by remember { mutableStateOf("") }
    var approvalBusy by remember { mutableStateOf(false) }
    var approvalError by remember { mutableStateOf<String?>(null) }
    var approvalNotice by remember { mutableStateOf<String?>(null) }
    var detailedNodeId by remember { mutableStateOf<String?>(null) }
    var retireTarget by remember { mutableStateOf<String?>(null) }
    var retireError by remember { mutableStateOf<String?>(null) }
    var resolveTarget by remember { mutableStateOf<JSONObject?>(null) }
    var dismissTarget by remember { mutableStateOf<JSONObject?>(null) }
    var resolveError by remember { mutableStateOf<String?>(null) }
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
                        val view = admin.memberView()
                        val members = view.optJSONArray("members").toObjectList()
                        val pending = view.optJSONArray("pending_retirements").toObjectList()
                        // Sync observations are optional context, but a failed
                        // request must be distinguishable from "no record": do
                        // not render every peer as "未同期" when the source
                        // itself could not be read.
                        val syncResult =
                            if (members.isEmpty()) {
                                null
                            } else {
                                runCatching { admin.call("sync-status") }
                            }
                        val sync = syncResult?.getOrNull()
                        val syncStatusError =
                            syncResult?.exceptionOrNull()?.let {
                                "同期記録を取得できませんでした: ${errorLabel(it)}"
                            }
                        val syncByNode =
                            sync?.optJSONArray("peers").toObjectList().associateBy {
                                it.optString("node")
                            }.orEmpty()
                        val self =
                            runCatching { admin.identity().optString("node_id") }
                                .getOrDefault("")
                        val peerFile = File(controller.nodeRoot, "peer.json")
                        val code =
                            if (peerFile.isFile) PeerCode.encode(peerFile.readBytes()) else null
                        PeersData(members, pending, syncByNode, syncStatusError, self, code)
                    } finally {
                        admin.close()
                    }
                }
            }.onSuccess { data ->
                members = data.members
                pendingRetirements = data.pendingRetirements
                syncByNode = data.syncByNode
                syncStatusError = data.syncStatusError
                selfNodeId = data.selfNodeId
                invite = data.invite
                error = null
            }.onFailure { error = errorLabel(it) }
            loading = false
        }
    }

    fun copy(label: String, value: String) {
        context.getSystemService(ClipboardManager::class.java)
            .setPrimaryClip(ClipData.newPlainText(label, value))
        Toast.makeText(context, "コピーしました", Toast.LENGTH_SHORT).show()
    }

    fun retire(nodeId: String) {
        val currentActivity = activity
        val body = Hex.decodeOrNull(nodeId)
        if (currentActivity == null || body == null) {
            retireError = "この画面から退役を実行できません。"
            return
        }
        busy = true
        retireError = null
        scope.launch {
            val result =
                runCatching {
                    withContext(Dispatchers.IO) {
                        val admin = controller.adminClient()
                        try {
                            application.ownerAdmin.call(
                                admin,
                                currentActivity,
                                "retire",
                                body,
                            )
                        } finally {
                            admin.close()
                        }
                    }
                }
            busy = false
            result
                .onSuccess {
                    retireTarget = null
                    notice = "ピアを退役しました。"
                    actionError = null
                    reload()
                }
                .onFailure { retireError = errorLabel(it) }
        }
    }

    fun resolveRetirement(certificateId: String, confirm: Boolean) {
        val currentActivity = activity
        val body = Hex.decodeOrNull(certificateId)
        if (currentActivity == null || body == null) {
            resolveError = "この画面から退役を確定できません。"
            return
        }
        busy = true
        resolveError = null
        scope.launch {
            val result =
                runCatching {
                    withContext(Dispatchers.IO) {
                        val admin = controller.adminClient()
                        try {
                            application.ownerAdmin.call(
                                admin,
                                currentActivity,
                                if (confirm) "confirm-retirement" else "dismiss-retirement",
                                body,
                            )
                        } finally {
                            admin.close()
                        }
                    }
                }
            busy = false
            result
                .onSuccess {
                    resolveTarget = null
                    dismissTarget = null
                    notice = if (confirm) "退役を確定しました。" else "退役を却下しました。"
                    actionError = null
                    reload()
                }
                .onFailure {
                    resolveError = errorLabel(it)
                    actionError = errorLabel(it)
                }
        }
    }

    LaunchedEffect(Unit) { reload() }

    val selfId = selfNodeId.ifBlank { snapshot?.nodeId.orEmpty() }
    val inflightIds = snapshot?.inflight?.toSet().orEmpty()
    val syncAvailable = syncStatusError == null
    val detailed =
        detailedNodeId?.let { id -> members.firstOrNull { it.optString("node_id") == id } }

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
                                color = Color.White,
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

        error?.let {
            item {
                Surface(
                    shape = MaterialTheme.shapes.medium,
                    color = MaterialTheme.colorScheme.errorContainer,
                    contentColor = MaterialTheme.colorScheme.onErrorContainer,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        it,
                        modifier = Modifier.padding(14.dp),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
        }

        notice?.let {
            item {
                Surface(
                    shape = MaterialTheme.shapes.medium,
                    color = HarmionSuccess.accentContainer,
                    contentColor = MaterialTheme.colorScheme.onSurface,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(it, modifier = Modifier.padding(14.dp), style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
        actionError?.let {
            item {
                Surface(
                    shape = MaterialTheme.shapes.medium,
                    color = MaterialTheme.colorScheme.errorContainer,
                    contentColor = MaterialTheme.colorScheme.onErrorContainer,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(it, modifier = Modifier.padding(14.dp), style = MaterialTheme.typography.bodyMedium)
                }
            }
        }

        syncStatusError?.let { message ->
            item {
                Surface(
                    shape = MaterialTheme.shapes.medium,
                    color = MaterialTheme.colorScheme.tertiaryContainer,
                    contentColor = MaterialTheme.colorScheme.onTertiaryContainer,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Row(
                        modifier = Modifier.padding(14.dp),
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Icon(
                            Icons.Rounded.WarningAmber,
                            contentDescription = null,
                            modifier = Modifier.size(20.dp),
                        )
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text("同期記録を取得できませんでした", style = MaterialTheme.typography.titleSmall)
                            Text(message, style = MaterialTheme.typography.bodySmall)
                            TextButton(onClick = { reload() }) { Text("再試行") }
                        }
                    }
                }
            }
        }

        if (pendingRetirements.isNotEmpty()) {
            item {
                SectionHeader("確認待ちの退役", "${pendingRetirements.size} 件の判断が必要です")
            }
            items(pendingRetirements, key = { it.optString("certificate_id") }) { pending ->
                PendingRetirementCard(
                    pending = pending,
                    busy = busy,
                    error = if (resolveTarget?.optString("certificate_id") ==
                        pending.optString("certificate_id")
                    ) {
                        resolveError
                    } else {
                        null
                    },
                    selfNodeId = selfId,
                    onConfirm = {
                        resolveError = null
                        resolveTarget = pending
                    },
                    onDismiss = {
                        resolveError = null
                        dismissTarget = pending
                    },
                )
            }
        }

        item {
            SectionHeader(
                "メンバー",
                "${members.size} 台のノード",
            )
        }

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
            val nodeId = member.optString("node_id")
            MemberCard(
                member = member,
                sync = syncByNode[nodeId],
                pending = pendingRetirements.firstOrNull {
                    it.optString("target") == nodeId
                },
                self = nodeId == selfId,
                inFlight = nodeId in inflightIds,
                syncAvailable = syncAvailable,
                onClick = {
                    retireError = null
                    resolveError = null
                    detailedNodeId = nodeId
                },
            )
        }

        item { androidx.compose.foundation.layout.Spacer(Modifier.size(8.dp)) }
    }

    detailed?.let { member ->
        PeerDetailSheet(
            member = member,
            sync = syncByNode[member.optString("node_id")],
            pending = pendingRetirements.firstOrNull {
                it.optString("target") == member.optString("node_id")
            },
            self = member.optString("node_id") == selfId,
            syncAvailable = syncAvailable,
            identityKnown = selfId.isNotBlank(),
            onDismiss = { detailedNodeId = null },
            onCopy = ::copy,
            onRetire = {
                detailedNodeId = null
                retireError = null
                retireTarget = member.optString("node_id")
            },
        )
    }

    retireTarget?.let { nodeId ->
        AlertDialog(
            onDismissRequest = { if (!busy) retireTarget = null },
            icon = {
                Icon(
                    Icons.Rounded.Block,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.error,
                )
            },
            title = { Text("このピアを退役させますか？") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        "「${shortId(nodeId)}」をクラスタから退役させます。",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    Text(
                        "この操作は取り消せません。",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    retireError?.let {
                        Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                    }
                }
            },
            confirmButton = {
                Button(
                    onClick = { retire(nodeId) },
                    enabled = !busy,
                    colors =
                        ButtonDefaults.buttonColors(
                            containerColor = MaterialTheme.colorScheme.error,
                            contentColor = MaterialTheme.colorScheme.onError,
                        ),
                ) {
                    if (busy) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(16.dp),
                            strokeWidth = 2.dp,
                            color = MaterialTheme.colorScheme.onError,
                        )
                        Spacer(Modifier.size(8.dp))
                    }
                    Text(if (busy) "退役中…" else "退役させる")
                }
            },
            dismissButton = {
                TextButton(onClick = { retireTarget = null }, enabled = !busy) { Text("キャンセル") }
            },
        )
    }

    resolveTarget?.let { pending ->
        val targetsSelf = pending.optString("target") == selfId
        AlertDialog(
            onDismissRequest = { if (!busy) resolveTarget = null },
            icon = {
                Icon(
                    Icons.Rounded.WarningAmber,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.error,
                )
            },
            title = {
                Text(if (targetsSelf) "この端末を退役させますか？" else "退役を確定しますか？")
            },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        "発行元 ${shortId(pending.optString("issuer"))} の退役届を確定します。" +
                            "対象 ${shortId(pending.optString("target"))} は退役します。",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    if (targetsSelf) {
                        Text(
                            "退役対象はこの端末自身です。確定するとこのノードはクラスタから離脱し、" +
                                "同じノードIDでは再参加できません。",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                    Text(
                        "この操作は取り消せません。",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                    resolveError?.let {
                        Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                    }
                }
            },
            confirmButton = {
                Button(
                    onClick = { resolveRetirement(pending.optString("certificate_id"), true) },
                    enabled = !busy,
                    colors =
                        ButtonDefaults.buttonColors(
                            containerColor = MaterialTheme.colorScheme.error,
                            contentColor = MaterialTheme.colorScheme.onError,
                        ),
                ) {
                    if (busy) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(16.dp),
                            strokeWidth = 2.dp,
                            color = MaterialTheme.colorScheme.onError,
                        )
                        Spacer(Modifier.size(8.dp))
                    }
                    Text(
                        when {
                            busy -> "確定中…"
                            targetsSelf -> "この端末を退役させる"
                            else -> "確定する"
                        },
                    )
                }
            },
            dismissButton = {
                TextButton(onClick = { resolveTarget = null }, enabled = !busy) { Text("キャンセル") }
            },
        )
    }

    dismissTarget?.let { pending ->
        AlertDialog(
            onDismissRequest = { if (!busy) dismissTarget = null },
            title = { Text("退役を却下しますか？") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        "発行元 ${shortId(pending.optString("issuer"))} の退役届を却下します。" +
                            "対象 ${shortId(pending.optString("target"))} の退役は無効として記録されます。",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    Text(
                        "この判断は取り消せません。",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    resolveError?.let {
                        Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                    }
                }
            },
            confirmButton = {
                Button(
                    onClick = { resolveRetirement(pending.optString("certificate_id"), false) },
                    enabled = !busy,
                ) {
                    if (busy) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(16.dp),
                            strokeWidth = 2.dp,
                            color = MaterialTheme.colorScheme.onPrimary,
                        )
                        Spacer(Modifier.size(8.dp))
                    }
                    Text(if (busy) "処理中…" else "却下する")
                }
            },
            dismissButton = {
                TextButton(onClick = { dismissTarget = null }, enabled = !busy) { Text("キャンセル") }
            },
        )
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

@Composable
private fun MemberCard(
    member: JSONObject,
    sync: JSONObject?,
    pending: JSONObject?,
    self: Boolean,
    inFlight: Boolean,
    syncAvailable: Boolean,
    onClick: () -> Unit,
) {
    val nodeId = member.optString("node_id")
    val state = member.optString("state")
    val syncLine = peerSyncLine(state, self, sync, inFlight, syncAvailable)
    val (container, content) = memberStateColors(state)
    HarmionCard(
        modifier =
            Modifier.fillMaxWidth().clickable(
                onClickLabel = "ピアの詳細を表示",
                onClick = onClick,
            ),
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Surface(
                shape = MaterialTheme.shapes.medium,
                color = container,
                contentColor = content,
            ) {
                Icon(
                    Icons.Rounded.Group,
                    contentDescription = null,
                    modifier = Modifier.padding(10.dp).size(20.dp),
                )
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Text(
                        shortId(nodeId),
                        style = MaterialTheme.typography.titleSmall,
                        fontFamily = FontFamily.Monospace,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                    if (self) {
                        Text(
                            "この端末",
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.primary,
                        )
                    }
                }
                syncLine?.let {
                    Text(
                        it.text,
                        style = MaterialTheme.typography.bodySmall,
                        color =
                            if (it.isError) {
                                MaterialTheme.colorScheme.error
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                    )
                }
                StatusPill(memberStateLabel(state), container = container, contentColor = content)
                if (pending != null) {
                    Text(
                        "退役の確認待ち",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
            Icon(
                Icons.AutoMirrored.Rounded.KeyboardArrowRight,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(20.dp),
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PeerDetailSheet(
    member: JSONObject,
    sync: JSONObject?,
    pending: JSONObject?,
    self: Boolean,
    syncAvailable: Boolean,
    identityKnown: Boolean,
    onDismiss: () -> Unit,
    onCopy: (String, String) -> Unit,
    onRetire: () -> Unit,
) {
    val nodeId = member.optString("node_id")
    val state = member.optString("state")
    val (container, content) = memberStateColors(state)
    val successAt = sync?.optString("last_success_at")?.toLongOrNull()
    val failure = sync?.optJSONObject("last_failure")
    val failureAt = failure?.optString("attempted_at")?.toLongOrNull()
    val failureCode = failure?.optString("code")
    val unconfirmedLocal = sync?.optString("unconfirmed_local")?.toIntOrNull()
    val sheetState = rememberBottomSheetState(initialValue = SheetValue.Hidden)
    var retireExpanded by remember(nodeId) { mutableStateOf(false) }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState) {
        Column(
            modifier =
                Modifier.fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(
                        "ピアの詳細",
                        style = MaterialTheme.typography.labelLarge,
                        color = MaterialTheme.colorScheme.primary,
                    )
                    Text(
                        shortId(nodeId),
                        style = MaterialTheme.typography.headlineSmall,
                        fontFamily = FontFamily.Monospace,
                    )
                }
                StatusPill(memberStateLabel(state), container = container, contentColor = content)
            }

            Text(
                "ノードID",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Surface(
                shape = MaterialTheme.shapes.small,
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                modifier = Modifier.fillMaxWidth(),
            ) {
                SelectionContainer {
                    Text(
                        nodeId,
                        modifier = Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                    )
                }
            }
            TextButton(onClick = { onCopy("ノードID", nodeId) }) {
                Text("ノードIDをコピー")
            }

            HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)

            InfoRow("状態", memberStateLabel(state))
            if (!self) {
                InfoRow(
                    "最終同期",
                    when {
                        !syncAvailable -> "取得できません"
                        successAt != null -> absoluteMicros(successAt)
                        else -> "記録なし"
                    },
                )
                if (failureAt != null) {
                    InfoRow(
                        "最終失敗",
                        "${syncFailureLabel(failureCode)}（${absoluteMicros(failureAt)}）",
                    )
                }
                unconfirmedLocal?.let {
                    InfoRow("相手が未確認の更新", "${it} 件")
                }
                sync?.optString("acknowledged_through")?.takeIf { it.isNotBlank() }?.let {
                    InfoRow("相手が確認済みの位置", it)
                }
                sync?.optString("remote_high")?.takeIf { it.isNotBlank() }?.let {
                    InfoRow("相手の最新位置", it)
                }
            }
            pending?.let {
                InfoRow("退役の確認", "このピア宛ての退役届があります")
            }

            if (state != "retired" && !self && identityKnown) {
                HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                if (retireExpanded) {
                    Button(
                        onClick = onRetire,
                        modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp),
                        colors =
                            ButtonDefaults.buttonColors(
                                containerColor = MaterialTheme.colorScheme.error,
                                contentColor = MaterialTheme.colorScheme.onError,
                            ),
                    ) {
                        Icon(Icons.Rounded.Block, contentDescription = null, modifier = Modifier.size(18.dp))
                        Text("このピアを退役させる", modifier = Modifier.padding(start = 8.dp))
                    }
                } else {
                    TextButton(
                        onClick = { retireExpanded = true },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text("退役")
                        Spacer(Modifier.weight(1f))
                        Icon(
                            Icons.Rounded.ExpandMore,
                            contentDescription = null,
                            modifier = Modifier.size(18.dp),
                        )
                    }
                }
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}

@Composable
private fun PendingRetirementCard(
    pending: JSONObject,
    busy: Boolean,
    error: String?,
    selfNodeId: String,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
    val issuer = pending.optString("issuer")
    val target = pending.optString("target")
    HarmionCard(containerColor = MaterialTheme.colorScheme.errorContainer) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Icon(Icons.Rounded.WarningAmber, contentDescription = null)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text("退役の確認待ち", style = MaterialTheme.typography.titleSmall)
                Text(
                    "発行元 ${shortId(issuer)} → 対象 ${shortId(target)}",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        if (target == selfNodeId) {
            Text(
                "この端末が退役対象です。",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }
        error?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            OutlinedButton(
                onClick = onDismiss,
                enabled = !busy,
                modifier = Modifier.weight(1f),
            ) { Text(if (busy) "処理中…" else "却下") }
            Button(
                onClick = onConfirm,
                enabled = !busy,
                modifier = Modifier.weight(1f),
                colors =
                    ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.error,
                        contentColor = MaterialTheme.colorScheme.onError,
                    ),
            ) { Text("確定") }
        }
    }
}

private fun peerSyncLine(
    state: String,
    self: Boolean,
    sync: JSONObject?,
    inFlight: Boolean,
    syncAvailable: Boolean,
): PeerSyncLine? {
    // This node is not its own peer; a sync record for it would always be empty.
    if (self || state != "active") return null
    if (inFlight) return PeerSyncLine("同期中", isError = false)
    if (!syncAvailable) return null
    val data = sync ?: return null
    val successAt = data.optString("last_success_at").toLongOrNull()
    val failureAt =
        data.optJSONObject("last_failure")?.optString("attempted_at")?.toLongOrNull()
    return when {
        failureAt != null && (successAt == null || failureAt > successAt) ->
            PeerSyncLine("最終失敗 ${absoluteMicros(failureAt)}", isError = true)
        successAt != null -> PeerSyncLine("最終同期 ${absoluteMicros(successAt)}", isError = false)
        else -> PeerSyncLine("同期記録なし", isError = false)
    }
}

@Composable
private fun memberStateColors(state: String): Pair<Color, Color> =
    when (state) {
        "active" ->
            MaterialTheme.colorScheme.primaryContainer to
                MaterialTheme.colorScheme.onPrimaryContainer
        "retired", "conflict" ->
            MaterialTheme.colorScheme.errorContainer to
                MaterialTheme.colorScheme.onErrorContainer
        "unconfirmed" ->
            MaterialTheme.colorScheme.tertiaryContainer to
                MaterialTheme.colorScheme.onTertiaryContainer
        else ->
            MaterialTheme.colorScheme.surfaceContainerHighest to
                MaterialTheme.colorScheme.onSurfaceVariant
    }

private fun JSONArray?.toObjectList(): List<JSONObject> {
    if (this == null) return emptyList()
    return (0 until length()).mapNotNull { index ->
        optJSONObject(index)
    }
}
