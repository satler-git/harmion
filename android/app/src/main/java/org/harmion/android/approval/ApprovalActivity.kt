package org.harmion.android.approval

import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.fragment.app.FragmentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.lifecycleScope
import android.view.WindowManager
import java.security.MessageDigest
import java.text.DateFormat
import java.util.Date
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.harmion.android.HarmionApplication
import org.harmion.android.ui.theme.HarmionTheme
import org.harmion.sdk.Hex
import org.harmion.sdk.NodeClient
import org.json.JSONObject

/**
 * Dialog-style approval activity.
 *
 * The launching app is verified twice: the bound service issued the token to the
 * Binder caller's UID/package/signing certificate, and this activity checks the
 * package Android reports as the launcher. A token alone cannot approve a
 * request for a different app.
 */
class ApprovalActivity : FragmentActivity() {
    private var previousGrants by mutableStateOf<org.json.JSONObject?>(null)

    private val connectionId: String? by lazy {
        intent.getStringExtra(NodeClient.EXTRA_CONNECTION_ID)
    }
    private val token: String? by lazy {
        intent.getStringExtra(NodeClient.EXTRA_APPROVAL_TOKEN)
    }
    private var tokenValid = false

    private var request by mutableStateOf<JSONObject?>(null)
    private var appLabel by mutableStateOf<String?>(null)
    private var error by mutableStateOf<String?>(null)
    private var loaded by mutableStateOf(false)
    private var deciding by mutableStateOf(false)
    private var grant: ApprovalRegistry.Grant? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // The approval screen can grant access to all data and app management:
        // block screenshots/recents and touches that pass through overlays.
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        window.decorView.filterTouchesWhenObscured = true
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            runCatching { window.setHideOverlayWindows(true) }
        }
        val pending = token?.let { ApprovalRegistry.peek(it) }
        @Suppress("DEPRECATION")
        val launchedFrom = getLaunchedFromPackage()
        error =
            when {
                connectionId == null || token == null || pending == null ->
                    "承認要求が見つからないか、有効期限が切れています。"
                pending.connectionId != connectionId -> "承認要求が一致しません。"
                launchedFrom == null -> "要求元アプリを確認できません。"
                launchedFrom != pending.packageName -> "要求元アプリが承認要求と一致しません。"
                else -> null
            }
        grant = pending
        tokenValid = error == null
        setContent {
            HarmionTheme {
                Scaffold(modifier = Modifier.fillMaxSize()) { padding ->
                    ApprovalScreen(
                        request = request,
                        appLabel = appLabel,
                        grant = grant,
                        previousGrants = previousGrants,
                        error = error,
                        loaded = loaded,
                        deciding = deciding,
                        onAllow = { days -> decide(true, days) },
                        onDeny = { if (tokenValid) decide(false) else finish() },
                        modifier = Modifier.padding(padding),
                    )
                }
            }
        }
        if (tokenValid) loadRequest()
    }

    private fun certificateSha256(packageName: String): String? =
        runCatching {
            val info =
                packageManager.getPackageInfo(
                    packageName,
                    PackageManager.GET_SIGNING_CERTIFICATES,
                )
            val signer = info.signingInfo?.apkContentsSigners?.firstOrNull()
                ?: return@runCatching null
            Hex.encode(MessageDigest.getInstance("SHA-256").digest(signer.toByteArray()))
        }.getOrNull()
    private fun loadRequest() {
        val id = connectionId ?: return
        val packageName = token?.let { ApprovalRegistry.peek(it)?.packageName }.orEmpty()
        lifecycleScope.launch {
            val result =
                withContext(Dispatchers.IO) {
                    runCatching {
                        val admin = (application as HarmionApplication).runtime.adminClient()
                        try {
                            val request =
                                admin.connections().firstOrNull {
                                    it.optString("connection_id") == id
                                } ?: return@runCatching null
                            val label =
                                runCatching {
                                    val info =
                                        packageManager.getPackageInfo(packageName, 0)
                                    packageManager.getApplicationLabel(requireNotNull(info.applicationInfo)).toString()
                                }.getOrNull()
                            val appId =
                                request.optString("app_id").ifEmpty {
                                    request.optString("client_id")
                                }
                            val existing =
                                if (appId.isEmpty()) {
                                    null
                                } else {
                                    admin.apps()
                                        .firstOrNull { it.optString("app_id") == appId }
                                        ?.optJSONObject("grants")
                                }
                            Triple(request, label, existing)
                        } finally {
                            admin.close()
                        }
                    }
                }
            loaded = true
            result
                .onSuccess { value ->
                    if (value == null) {
                        error = "保留中の承認要求が見つかりません。"
                    } else {
                        request = value.first
                        appLabel = value.second
                        previousGrants = value.third
                    }
                }
                .onFailure { error = it.message ?: "承認要求を読み込めませんでした。" }
        }
    }

    private fun decide(allow: Boolean, expiryDays: Int = 30) {
        if (deciding) return
        val currentToken = token ?: return
        val pending = ApprovalRegistry.peek(currentToken) ?: return
        // Re-check the signing certificate at the moment of approval: the
        // token carries the certificate the service verified, and a package
        // update must not silently change the app behind the approval.
        if (certificateSha256(pending.packageName) != pending.certificateSha256) {
            error = "要求元アプリの署名が承認要求と一致しません。"
            return
        }
        val grant = ApprovalRegistry.consume(currentToken) ?: return
        deciding = true
        lifecycleScope.launch {
            val result =
                withContext(Dispatchers.IO) {
                    runCatching {
                        val admin = (application as HarmionApplication).runtime.adminClient()
                        try {
                            val harmony = application as HarmionApplication
                            val expiresAt =
                                if (allow) {
                                    ((System.currentTimeMillis() + expiryDays * 86_400_000L) * 1000).toString()
                                } else {
                                    null
                                }
                            harmony.ownerAdmin.decideConnection(
                                admin,
                                this@ApprovalActivity,
                                grant.connectionId,
                                allow,
                                expiresAt,
                            )
                        } finally {
                            admin.close()
                        }
                    }
                }
            result
                .onSuccess {
                    setResult(if (allow) RESULT_OK else RESULT_CANCELED)
                    finish()
                }
                .onFailure {
                    error = it.message ?: "承認を記録できませんでした。"
                    deciding = false
                }
        }
    }
}

@Composable
private fun ApprovalScreen(
    request: JSONObject?,
    appLabel: String?,
    grant: ApprovalRegistry.Grant?,
    previousGrants: org.json.JSONObject?,
    error: String?,
    loaded: Boolean,
    deciding: Boolean,
    onAllow: (Int) -> Unit,
    onDeny: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier =
            modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("接続の承認", style = MaterialTheme.typography.headlineSmall)
        when {
            error != null -> {
                Text(error, color = MaterialTheme.colorScheme.error)
                Button(onClick = onDeny, modifier = Modifier.fillMaxWidth()) { Text("閉じる") }
            }
            request == null && !loaded -> CircularProgressIndicator()
            request == null -> Text("要求を読み込めませんでした。")
            else -> {
                Card {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(
                            request.optString("name", "アプリ"),
                            style = MaterialTheme.typography.titleLarge,
                            fontWeight = FontWeight.SemiBold,
                        )
                        Text(
                            "表示名は要求元が設定",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Text("パッケージ: ${grant?.packageName.orEmpty()}", style = MaterialTheme.typography.bodySmall)
                        Text(
                            "署名証明書: ${grant?.certificateSha256?.take(16).orEmpty()}…",
                            style = MaterialTheme.typography.bodySmall,
                        )
                        appLabel?.let {
                            Text("要求元アプリ: $it", style = MaterialTheme.typography.bodyMedium)
                        }
                        request.optString("redirect_uri").takeIf { it.isNotEmpty() }?.let {
                            Text("リダイレクト先: $it", style = MaterialTheme.typography.bodySmall)
                        }
                        request.optString("web_origin").takeIf { it.isNotEmpty() }?.let {
                            Text("Web origin: $it", style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
                Card {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text("要求された権限", style = MaterialTheme.typography.titleMedium)
                        grantSummary(request.optJSONObject("grants")).forEach {
                            Text("・$it", style = MaterialTheme.typography.bodyMedium)
                        }
                        val requested = request.optJSONObject("grants")
                        if (requested?.optBoolean("manage_apps") == true ||
                            requested?.optBoolean("manage_nodes") == true
                        ) {
                            Text(
                                "強い権限を要求しています",
                                color = MaterialTheme.colorScheme.error,
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                        val providers = request.optJSONArray("provider_names")
                        if (providers != null && providers.length() > 0) {
                            Text("Interpreter提供", style = MaterialTheme.typography.titleMedium)
                            for (index in 0 until providers.length()) {
                                Text(
                                    "・${providers.getString(index)}",
                                    style = MaterialTheme.typography.bodyMedium,
                                )
                            }
                        }
                        previousGrants?.let { before ->
                            val requested = grantSummary(request.optJSONObject("grants")).toSet()
                            val existing = grantSummary(before).toSet()
                            val added = requested - existing
                            val removed = existing - requested
                            when {
                                added.isEmpty() && removed.isEmpty() ->
                                    Text(
                                        "権限は変更されません",
                                        style = MaterialTheme.typography.bodySmall,
                                    )
                                else -> {
                                    Text("前回の認可からの変更", style = MaterialTheme.typography.titleMedium)
                                    added.forEach {
                                        Text("+ $it", style = MaterialTheme.typography.bodySmall)
                                    }
                                    removed.forEach {
                                        Text("− $it", style = MaterialTheme.typography.bodySmall)
                                    }
                                }
                            }
                        }
                        request.optString("expires_at").toLongOrNull()?.let { micros ->
                            val date = Date(micros / 1000)
                            Text(
                                "要求の有効期限: ${DateFormat.getDateTimeInstance().format(date)}",
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
                var expiryDays by remember { mutableStateOf(30) }
                Text("認可期限", style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf(30 to "30日", 90 to "90日", 365 to "1年").forEach { (days, label) ->
                        FilterChip(
                            selected = expiryDays == days,
                            onClick = { expiryDays = days },
                            label = { Text(label) },
                        )
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    OutlinedButton(
                        onClick = onDeny,
                        modifier = Modifier.weight(1f),
                        enabled = !deciding,
                    ) { Text("拒否") }
                    Button(
                        onClick = { onAllow(expiryDays) },
                        modifier = Modifier.weight(1f),
                        enabled = !deciding,
                    ) { Text(if (deciding) "処理中…" else "許可") }
                }
            }
        }
    }
}

internal fun grantSummary(grants: JSONObject?): List<String> {
    if (grants == null) return emptyList()
    val summary = mutableListOf<String>()
    grants.optJSONObject("read")?.let { summary += "データの読み取り (${filterLabel(it)})" }
    grants.optJSONObject("write")?.let { summary += "データの書き込み (${filterLabel(it)})" }
    if (grants.optBoolean("call")) summary += "Interpreterの呼び出し"
    if (grants.optBoolean("provide")) summary += "Interpreterの提供"
    if (grants.optBoolean("manage_apps")) summary += "アプリ認可の管理"
    if (grants.optBoolean("manage_nodes")) summary += "ノード管理"
    return summary
}

private fun filterLabel(filter: JSONObject): String =
    when (filter.optString("type")) {
        "all" -> "すべて"
        "kinds" ->
            filter.optJSONArray("kinds")?.let { array ->
                (0 until array.length()).joinToString(", ") { array.getString(it) }
            } ?: "指定kind"
        else -> "不明"
    }
