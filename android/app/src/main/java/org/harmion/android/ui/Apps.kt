package org.harmion.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Apps
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Refresh
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.harmion.android.HarmionApplication
import org.harmion.android.core.NodeRuntimeController
import org.json.JSONObject

@Composable
fun AppsScreen(controller: NodeRuntimeController) {
    val context = LocalContext.current
    val application = context.applicationContext as HarmionApplication
    val activity = context as? androidx.fragment.app.FragmentActivity
    val scope = rememberCoroutineScope()
    var connections by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var apps by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var managing by remember { mutableStateOf<JSONObject?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var loading by remember { mutableStateOf(true) }

    fun reload() {
        loading = true
        scope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val admin = controller.adminClient()
                    try {
                        admin.connections() to admin.apps()
                    } finally {
                        admin.close()
                    }
                }
            }.onSuccess { (requests, credentials) ->
                connections = requests
                apps = credentials
                error = null
            }.onFailure { error = it.message }
            loading = false
        }
    }

    LaunchedEffect(Unit) { reload() }

    val pending = connections.filter { it.optString("status") == "pending" }
    val approved = connections.filter { it.optString("status") == "approved" }

    LazyColumn(
        modifier = Modifier.fillMaxWidth(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    SectionHeader("接続アプリ", "承認待ち ${pending.size} 件 / 承認済み ${approved.size} 件")
                }
                IconButton(onClick = { reload() }) {
                    Icon(Icons.Rounded.Refresh, contentDescription = "再読み込み")
                }
            }
        }

        if (loading && connections.isEmpty()) {
            item { CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp) }
        }

        if (pending.isNotEmpty()) {
            item { SectionHeader("承認待ち") }
            items(pending, key = { it.optString("connection_id") }) { connection ->
                PendingCard(
                    connection = connection,
                    busy = busy,
                    onAllow = { allow ->
                        if (busy) return@PendingCard
                        busy = true
                        scope.launch {
                            runCatching {
                                withContext(Dispatchers.IO) {
                                    val admin = controller.adminClient()
                                    try {
                                        application.ownerAdmin.decideConnection(
                                            admin,
                                            activity,
                                            connection.getString("connection_id"),
                                            allow,
                                        )
                                    } finally {
                                        admin.close()
                                    }
                                }
                            }.onSuccess { reload() }
                                .onFailure { error = it.message }
                            busy = false
                        }
                    },
                )
            }
        }

        val visibleApps =
            apps.filterNot { app ->
                val grants = app.optJSONObject("grants")
                app.isNull("name") &&
                    grants?.optBoolean("manage_nodes") == true &&
                    grants.optBoolean("manage_apps")
            }
        if (visibleApps.isNotEmpty()) {
            item { SectionHeader("認可済みアプリ", "権限の変更は所有者承認が必要です") }
            items(visibleApps, key = { it.optString("app_id") }) { app ->
                AppCredentialCard(app = app, onManage = { managing = app })
            }
        }

        if (approved.isNotEmpty()) {
            item { SectionHeader("承認済み") }
            items(approved, key = { it.optString("connection_id") }) { connection ->
                HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Surface(
                            shape = MaterialTheme.shapes.medium,
                            color = MaterialTheme.colorScheme.tertiaryContainer,
                        ) {
                            Icon(
                                Icons.Rounded.CheckCircle,
                                contentDescription = null,
                                modifier = Modifier.padding(10.dp).size(20.dp),
                            )
                        }
                        Column(Modifier.weight(1f)) {
                            Text(
                                connection.optString("name", "アプリ"),
                                style = MaterialTheme.typography.titleSmall,
                            )
                            Text(
                                "接続 ${shortId(connection.optString("connection_id"))}",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        StatusPill("承認済み")
                    }
                    val grants = connection.optJSONObject("grants")
                    grantSummary(grants).forEach {
                        Text("・$it", style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        }

        if (!loading && connections.isEmpty()) {
            item {
                EmptyState(
                    icon = Icons.Rounded.Apps,
                    title = "接続中のアプリはありません",
                    body = "接続要求がここに表示されます。",
                )
            }
        }

        error?.let {
            item {
                HarmionCard(containerColor = MaterialTheme.colorScheme.errorContainer) {
                    Text(it, style = MaterialTheme.typography.bodyMedium)
                }
            }
        }

        item { androidx.compose.foundation.layout.Spacer(Modifier.size(8.dp)) }
    }

    managing?.let { app ->
        AppManageSheet(
            app = app,
            onDismiss = { managing = null },
            onSave = { grants, revoked, expiresAt ->
                scope.launch {
                    runCatching {
                        withContext(Dispatchers.IO) {
                            val admin = controller.adminClient()
                            try {
                                val body =
                                    JSONObject()
                                        .put("app_id", app.getString("app_id"))
                                        .put("grants", grants)
                                        .put("revoked", revoked)
                                        .apply { expiresAt?.let { put("expires_at", it) } }
                                application.ownerAdmin.call(
                                    admin,
                                    activity,
                                    "change-grants",
                                    body.toString().toByteArray(Charsets.UTF_8),
                                )
                            } finally {
                                admin.close()
                            }
                        }
                    }.onSuccess {
                        if (revoked) {
                            org.harmion.android.service.CallerAccess.revokeApp(
                                context,
                                app.optString("app_id"),
                            )
                        }
                        managing = null
                        reload()
                    }
                        .onFailure { error = it.message }
                }
            },
        )
    }
}

@Composable
private fun PendingCard(
    connection: JSONObject,
    busy: Boolean,
    onAllow: (Boolean) -> Unit,
) {
    HarmionCard {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Surface(
                shape = MaterialTheme.shapes.medium,
                color = MaterialTheme.colorScheme.primaryContainer,
            ) {
                Icon(
                    Icons.Rounded.Apps,
                    contentDescription = null,
                    modifier = Modifier.padding(10.dp).size(20.dp),
                )
            }
            Column(Modifier.weight(1f)) {
                Text(
                    connection.optString("name", "アプリ"),
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    "接続要求 ${shortId(connection.optString("connection_id"))}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            StatusPill("承認待ち")
        }
        val grants = connection.optJSONObject("grants")
        val lines = grantSummary(grants)
        if (lines.isEmpty()) {
            Text(
                "権限の詳細を取得できませんでした。",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        } else {
            lines.forEach { Text("・$it", style = MaterialTheme.typography.bodySmall) }
        }
        if (grants?.optBoolean("manage_apps") == true || grants?.optBoolean("manage_nodes") == true) {
            Text(
                "強い権限を要求しています",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            OutlinedButton(
                onClick = { onAllow(false) },
                enabled = !busy,
                modifier = Modifier.weight(1f),
            ) { Text("拒否") }
            Button(
                onClick = { onAllow(true) },
                enabled = !busy,
                modifier = Modifier.weight(1f),
            ) { Text("許可") }
        }
    }
}

@Composable
private fun AppCredentialCard(app: JSONObject, onManage: () -> Unit) {
    val grants = app.optJSONObject("grants")
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
                    Icons.Rounded.Apps,
                    contentDescription = null,
                    modifier = Modifier.padding(10.dp).size(20.dp),
                )
            }
            Column(Modifier.weight(1f)) {
                Text(
                    if (app.isNull("name") || app.optString("name").isEmpty()) "アプリ" else app.optString("name"),
                    style = MaterialTheme.typography.titleSmall,
                )
                Text(
                    "ID ${shortId(app.optString("app_id"))}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            StatusPill(if (app.optBoolean("revoked")) "失効" else "有効")
        }
        app.optString("expires_at").toLongOrNull()?.let {
            InfoRow("認可期限", if (it > Long.MAX_VALUE / 2) "無期限" else absoluteMicros(it))
        }
        grantSummary(grants).forEach {
            Text("・$it", style = MaterialTheme.typography.bodySmall)
        }
        OutlinedButton(onClick = onManage, modifier = Modifier.fillMaxWidth()) {
            Icon(Icons.Rounded.Refresh, contentDescription = null, modifier = Modifier.size(18.dp))
            Text("権限を管理", modifier = Modifier.padding(start = 8.dp))
        }
    }
}

@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun AppManageSheet(
    app: JSONObject,
    onDismiss: () -> Unit,
    onSave: (JSONObject, Boolean, String?) -> Unit,
) {
    val grants =
        remember(app) {
            app.optJSONObject("grants")?.let { JSONObject(it.toString()) } ?: JSONObject()
        }
    var call by remember(app) { mutableStateOf(grants.optBoolean("call")) }
    var provide by remember(app) { mutableStateOf(grants.optBoolean("provide")) }
    var manageApps by remember(app) { mutableStateOf(grants.optBoolean("manage_apps")) }
    var manageNodes by remember(app) { mutableStateOf(grants.optBoolean("manage_nodes")) }
    var revoked by remember(app) { mutableStateOf(app.optBoolean("revoked")) }
    var extension by remember(app) { mutableStateOf(0) }

    androidx.compose.material3.ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState =
            androidx.compose.material3.rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(
            modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(
                if (app.isNull("name") || app.optString("name").isEmpty()) "アプリ" else app.optString("name"),
                style = MaterialTheme.typography.headlineSmall,
            )
            Text(
                "所有者の生体認証で確定します",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            SheetSwitch("Interpreterを呼び出す", call) { call = it }
            SheetSwitch("Interpreterを提供する", provide) { provide = it }
            SheetSwitch("他のアプリの認可を管理", manageApps) { manageApps = it }
            SheetSwitch("ノード管理", manageNodes) { manageNodes = it }
            SheetSwitch("この認可を失効する", revoked) { revoked = it }
            Text("認可期限", style = MaterialTheme.typography.labelLarge)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf(0 to "変更なし", 30 to "30日", 90 to "90日", 365 to "1年").forEach { (days, label) ->
                    androidx.compose.material3.FilterChip(
                        selected = extension == days,
                        onClick = { extension = days },
                        label = { Text(label) },
                    )
                }
            }
            Row(
                modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                OutlinedButton(onClick = onDismiss, modifier = Modifier.weight(1f)) { Text("キャンセル") }
                Button(
                    onClick = {
                        grants.put("call", call)
                        grants.put("provide", provide)
                        grants.put("manage_apps", manageApps)
                        grants.put("manage_nodes", manageNodes)
                        val expiresAt =
                            if (extension > 0) {
                                ((System.currentTimeMillis() + extension * 86_400_000L) * 1000).toString()
                            } else {
                                null
                            }
                        onSave(grants, revoked, expiresAt)
                    },
                    modifier = Modifier.weight(1f),
                ) { Text("保存") }
            }
            androidx.compose.foundation.layout.Spacer(Modifier.size(24.dp))
        }
    }
}

@Composable
private fun SheetSwitch(label: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(label, modifier = Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
        androidx.compose.material3.Switch(checked = checked, onCheckedChange = onChange)
    }
}
