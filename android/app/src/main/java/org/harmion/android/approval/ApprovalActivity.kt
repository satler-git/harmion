package org.harmion.android.approval

import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.compose.setContent
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Apps
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.Shield
import androidx.compose.material.icons.rounded.WarningAmber
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.graphics.drawable.toBitmap
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.lifecycleScope
import java.security.MessageDigest
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.harmion.android.HarmionApplication
import org.harmion.android.service.CallerAccess
import org.harmion.android.ui.HarmionCard
import org.harmion.android.ui.SectionHeader
import org.harmion.android.ui.absoluteMicros
import org.harmion.android.ui.errorLabel
import org.harmion.android.ui.grantSummary
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
    private var appIcon by mutableStateOf<ImageBitmap?>(null)
    private var error by mutableStateOf<String?>(null)
    private var actionError by mutableStateOf<String?>(null)
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
                        appIcon = appIcon,
                        grant = grant,
                        previousGrants = previousGrants,
                        error = error,
                        actionError = actionError,
                        loaded = loaded,
                        deciding = deciding,
                        onAllow = { days -> decide(true, days) },
                        onDeny = {
                            val current = token
                            if (tokenValid && current != null && ApprovalRegistry.peek(current) != null) {
                                decide(false)
                            } else {
                                setResult(RESULT_CANCELED)
                                finishAndRemoveTask()
                            }
                        },
                        onClose = {
                            setResult(RESULT_CANCELED)
                            finishAndRemoveTask()
                        },
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
                            // The launcher app's icon is the strongest visual cue
                            // that this approval belongs to the app the user just
                            // used. A failure only falls back to a generic icon.
                            val icon =
                                runCatching {
                                    packageManager
                                        .getApplicationIcon(packageName)
                                        .toBitmap(width = 144, height = 144)
                                        .asImageBitmap()
                                }.getOrNull()
                            val appId =
                                request.optString("app_id").ifEmpty {
                                    request.optString("client_id")
                                }
                            val existing =
                                if (appId.isEmpty()) {
                                    null
                                } else {
                                    // The grant comparison is helpful context, not
                                    // a prerequisite for showing the approval.
                                    runCatching {
                                            admin.apps()
                                                .firstOrNull { it.optString("app_id") == appId }
                                                ?.optJSONObject("grants")
                                        }
                                        .getOrNull()
                                }
                            LoadedApproval(request, label, existing, icon)
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
                        request = value.request
                        appLabel = value.label
                        appIcon = value.icon
                        previousGrants = value.previousGrants
                    }
                }
                .onFailure { error = errorLabel(it) }
        }
    }

    private fun decide(allow: Boolean, expiryDays: Int = 30) {
        if (deciding) return
        actionError = null
        val currentToken =
            token
                ?: run {
                    error = "承認要求を読み込めませんでした。"
                    return
                }
        val pending =
            ApprovalRegistry.peek(currentToken)
                ?: run {
                    error = "承認の有効期限が切れました。"
                    return
                }
        // Re-check the signing certificate at the moment of approval: the
        // token carries the certificate the service verified, and a package
        // update must not silently change the app behind the approval.
        if (certificateSha256(pending.packageName) != pending.certificateSha256) {
            error = "要求元アプリの署名が承認要求と一致しません。"
            return
        }
        // Keep the one-time token until the node confirms the decision. A
        // transient transport error or a cancelled biometric prompt must leave
        // the request retryable instead of dead-ending on "expired".
        val grant = pending
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
                            val result =
                                harmony.ownerAdmin.decideConnection(
                                    admin,
                                    this@ApprovalActivity,
                                    grant.connectionId,
                                    allow,
                                    expiresAt,
                                )
                            val status = result.optString("status")
                            if (allow && status != "approved") {
                                throw IllegalStateException("承認を完了できませんでした。")
                            }
                            if (!allow && status != "denied") {
                                throw IllegalStateException("拒否を完了できませんでした。")
                            }
                            if (allow) {
                                CallerAccess.record(
                                    this@ApprovalActivity,
                                    pending.packageName,
                                    pending.certificateSha256,
                                    result.optString("app_id"),
                                )
                            }
                            result
                        } finally {
                            admin.close()
                        }
                    }
                }
            result
                .onSuccess {
                    ApprovalRegistry.consume(currentToken)
                    setResult(if (allow) RESULT_OK else RESULT_CANCELED)
                    // The approval runs in its own task (taskAffinity="") so the
                    // requesting app returns to the foreground instead of the
                    // node app's main screen.
                    finishAndRemoveTask()
                }
                .onFailure {
                    actionError = errorLabel(it)
                    deciding = false
                }
        }
    }
}

private data class LoadedApproval(
    val request: JSONObject,
    val label: String?,
    val previousGrants: JSONObject?,
    val icon: ImageBitmap?,
)

/**
 * Approval is a full-screen decision, not a dialog: the top half answers "who
 * is asking and what can they do", and the sticky footer holds the irreversible
 * decision. Every card shares the same horizontal inset so the surface edges
 * line up even when the content wraps.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ApprovalScreen(
    request: JSONObject?,
    appLabel: String?,
    appIcon: ImageBitmap?,
    grant: ApprovalRegistry.Grant?,
    previousGrants: JSONObject?,
    error: String?,
    actionError: String?,
    loaded: Boolean,
    deciding: Boolean,
    onAllow: (Int) -> Unit,
    onDeny: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var expiryDays by rememberSaveable { mutableStateOf(30) }
    val canAllow = request != null && request.optJSONObject("grants") != null
    Column(modifier = modifier.fillMaxSize()) {
        Column(
            modifier =
                Modifier.weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            ApprovalHeader(
                appLabel = appLabel,
                appIcon = appIcon,
                grant = grant,
            )
            when {
                error != null -> ErrorPanel(error, onClose)
                request == null && !loaded -> LoadingPanel()
                request == null -> ErrorPanel("要求を読み込めませんでした。", onClose)
                else -> {
                    IdentityCard(appLabel = appLabel, grant = grant)
                    PermissionsCard(request = request)
                    RequestDetailsCard(request = request)
                    if (canAllow) {
                        PermissionDeltaCard(request = request, previousGrants = previousGrants)
                    }
                    RequestExpiryCard(request = request)
                    Spacer(Modifier.height(4.dp))
                }
            }
        }
        actionError?.let {
            ErrorBanner(
                it,
                Modifier.padding(start = 20.dp, end = 20.dp, bottom = 8.dp),
            )
        }
        if (request != null && error == null) {
            ApprovalActionBar(
                expiryDays = expiryDays,
                onExpiryChange = { expiryDays = it },
                canAllow = canAllow,
                deciding = deciding,
                onDeny = onDeny,
                onAllow = { onAllow(expiryDays) },
            )
        }
    }
}

@Composable
private fun ApprovalHeader(
    appLabel: String?,
    appIcon: ImageBitmap?,
    grant: ApprovalRegistry.Grant?,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Surface(
            shape = MaterialTheme.shapes.extraLarge,
            color = MaterialTheme.colorScheme.primaryContainer,
            contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
            modifier = Modifier.size(64.dp),
        ) {
            Box(Modifier.fillMaxSize().padding(6.dp), contentAlignment = Alignment.Center) {
                if (appIcon != null) {
                    Image(
                        bitmap = appIcon,
                        contentDescription = null,
                        contentScale = ContentScale.Fit,
                        modifier = Modifier.fillMaxSize(),
                    )
                } else {
                    Icon(
                        Icons.Rounded.Apps,
                        contentDescription = null,
                        modifier = Modifier.size(30.dp),
                    )
                }
            }
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                "接続の承認",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.primary,
            )
            Text(
                appLabel ?: "アプリ",
                style = MaterialTheme.typography.headlineSmall,
                fontWeight = FontWeight.SemiBold,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                grant?.packageName.orEmpty(),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun IdentityCard(
    appLabel: String?,
    grant: ApprovalRegistry.Grant?,
) {
    HarmionCard {
        SectionHeader(
            "要求元の確認",
            "Android が確認したパッケージと署名だけを表示します",
        )
        Surface(
            shape = MaterialTheme.shapes.medium,
            color = MaterialTheme.colorScheme.secondaryContainer,
            contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Row(
                modifier = Modifier.padding(12.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.Rounded.Shield, contentDescription = null, modifier = Modifier.size(20.dp))
                Text(
                    "この画面のタイトルとアイコンはAndroidが確認したアプリです。下のパッケージと署名を優先してください。",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        DetailField("パッケージ", grant?.packageName.orEmpty())
        DetailField(
            "署名証明書 (SHA-256)",
            grant?.certificateSha256.orEmpty(),
            monospace = true,
        )
        appLabel?.let { DetailField("ランチャーの表示名", it) }
    }
}

/**
 * Values chosen by the requesting app. They are useful context, but they are
 * not verified: a malicious app can put anything here, including another app's
 * name. Keep them visually separate from the verified package/signature.
 */
@Composable
private fun RequestDetailsCard(request: JSONObject) {
    val name = sanitizeUntrusted(request.optString("name"))
    val redirect = request.optString("redirect_uri").takeIf { it.isNotEmpty() }
    val webOrigin =
        request.optString("web_origin").takeIf { it.isNotEmpty() && it != "null" }
    if (name.isEmpty() && redirect == null && webOrigin == null) return
    HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        SectionHeader(
            "要求元が指定した情報",
            "未検証の表示です。上のパッケージと署名を確認してください",
        )
        if (name.isNotEmpty()) DetailField("表示名（未検証）", name)
        redirect?.let { DetailField("リダイレクト先", it) }
        webOrigin?.let { DetailField("Web origin", it) }
    }
}

private fun sanitizeUntrusted(raw: String): String {
    val cleaned =
        raw.filterNot { char ->
            char.isISOControl() ||
                char in '\u202A'..'\u202E' ||
                char in '\u2066'..'\u2069' ||
                char in '\u200E'..'\u200F' ||
                char == '\u061C'
        }.trim()
    return if (cleaned.length > 120) "${cleaned.take(120)}…" else cleaned
}

@Composable
private fun PermissionsCard(request: JSONObject) {
    val grants = request.optJSONObject("grants")
    val lines = grantSummary(grants)
    val strong =
        grants?.optBoolean("manage_apps") == true || grants?.optBoolean("manage_nodes") == true
    val providers = request.optJSONArray("provider_names")
    HarmionCard {
        SectionHeader("要求された権限")
        if (grants == null) {
            // Missing grants are not the same as an empty grant set. Refusing to
            // render "no permissions" prevents an unreadable request from looking
            // harmless; the allow button is disabled by the caller as well.
            Surface(
                shape = MaterialTheme.shapes.medium,
                color = MaterialTheme.colorScheme.errorContainer,
                contentColor = MaterialTheme.colorScheme.onErrorContainer,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Row(
                    modifier = Modifier.padding(12.dp),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Icon(
                        Icons.Rounded.ErrorOutline,
                        contentDescription = null,
                        modifier = Modifier.size(20.dp),
                    )
                    Text(
                        "権限情報を取得できませんでした。この要求は許可しないでください。",
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        } else if (lines.isEmpty()) {
            Text(
                "権限は要求されていません。",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        } else {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                lines.forEach { GrantLine(it) }
            }
        }
        if (strong) {
            Surface(
                shape = MaterialTheme.shapes.medium,
                color = MaterialTheme.colorScheme.errorContainer,
                contentColor = MaterialTheme.colorScheme.onErrorContainer,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Row(
                    modifier = Modifier.padding(12.dp),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Icon(
                        Icons.Rounded.WarningAmber,
                        contentDescription = null,
                        modifier = Modifier.size(20.dp),
                    )
                    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text("強い権限を要求しています", style = MaterialTheme.typography.titleSmall)
                        Text(
                            "アプリの認可やノード管理を操作できます。信頼できるアプリだけに許可してください。",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
        }
        if (providers != null && providers.length() > 0) {
            SectionHeader("Interpreter提供")
            for (index in 0 until providers.length()) {
                GrantLine(providers.getString(index))
            }
        }
    }
}

@Composable
private fun PermissionDeltaCard(
    request: JSONObject,
    previousGrants: JSONObject?,
) {
    if (previousGrants == null) return
    val requested = grantSummary(request.optJSONObject("grants")).toSet()
    val existing = grantSummary(previousGrants).toSet()
    val added = requested - existing
    val removed = existing - requested
    HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        SectionHeader("前回の認可からの変更")
        if (added.isEmpty() && removed.isEmpty()) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(
                    Icons.Rounded.CheckCircle,
                    contentDescription = null,
                    modifier = Modifier.size(18.dp),
                    tint = MaterialTheme.colorScheme.primary,
                )
                Text("権限は前回と同じです。", style = MaterialTheme.typography.bodyMedium)
            }
        }
        if (added.isNotEmpty()) {
            Text(
                "追加される権限",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.error,
            )
            added.sorted().forEach {
                Text("+ $it", style = MaterialTheme.typography.bodySmall)
            }
        }
        if (removed.isNotEmpty()) {
            Text("失われる権限", style = MaterialTheme.typography.labelLarge)
            removed.sorted().forEach {
                Text("− $it", style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

@Composable
private fun RequestExpiryCard(request: JSONObject) {
    val expiresAt = request.optString("expires_at").toLongOrNull() ?: return
    HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        SectionHeader("要求の有効期限", "この時刻を過ぎると、この承認は失効します")
        Text(
            absoluteMicros(expiresAt),
            style = MaterialTheme.typography.titleMedium,
        )
    }
}

@Composable
private fun ApprovalActionBar(
    expiryDays: Int,
    onExpiryChange: (Int) -> Unit,
    canAllow: Boolean,
    deciding: Boolean,
    onDeny: () -> Unit,
    onAllow: () -> Unit,
) {
    val options = listOf(30 to "30日", 90 to "90日", 365 to "1年")
    Surface(
        color = MaterialTheme.colorScheme.surfaceContainer,
        tonalElevation = 3.dp,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(
            modifier = Modifier.padding(horizontal = 20.dp, vertical = 14.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                "認可期限",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                options.forEachIndexed { index, (days, label) ->
                    SegmentedButton(
                        selected = expiryDays == days,
                        onClick = { onExpiryChange(days) },
                        enabled = !deciding,
                        shape = SegmentedButtonDefaults.itemShape(index = index, count = options.size),
                        label = { Text(label, maxLines = 1) },
                    )
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                OutlinedButton(
                    onClick = onDeny,
                    modifier = Modifier.weight(1f).heightIn(min = 52.dp),
                    enabled = !deciding,
                ) { Text("拒否") }
                Button(
                    onClick = onAllow,
                    modifier = Modifier.weight(1f).heightIn(min = 52.dp),
                    enabled = !deciding && canAllow,
                ) {
                    if (deciding) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(16.dp),
                            strokeWidth = 2.dp,
                            color = MaterialTheme.colorScheme.onPrimary,
                        )
                        Spacer(Modifier.size(8.dp))
                    }
                    Text(if (deciding) "処理中…" else "許可")
                }
            }
            Text(
                if (canAllow) {
                    "許可すると、このアプリは上記の権限でノードにアクセスできます。"
                } else {
                    "権限情報を取得できないため許可できません。"
                },
                style = MaterialTheme.typography.bodySmall,
                color =
                    if (canAllow) {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    } else {
                        MaterialTheme.colorScheme.error
                    },
            )
        }
    }
}

@Composable
private fun DetailField(
    label: String,
    value: String,
    monospace: Boolean = false,
) {
    if (value.isBlank()) return
    Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Text(
            label,
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        if (monospace) {
            Surface(
                shape = MaterialTheme.shapes.small,
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                modifier = Modifier.fillMaxWidth(),
            ) {
                SelectionContainer {
                    Text(
                        value,
                        modifier = Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                    )
                }
            }
        } else {
            Text(value, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable
private fun GrantLine(text: String) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(
            Icons.Rounded.CheckCircle,
            contentDescription = null,
            modifier = Modifier.size(18.dp),
            tint = MaterialTheme.colorScheme.primary,
        )
        Text(text, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
    }
}

@Composable
private fun ErrorPanel(message: String, onClose: () -> Unit) {
    Column(
        modifier = Modifier.fillMaxWidth().padding(vertical = 32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Surface(
            shape = MaterialTheme.shapes.extraLarge,
            color = MaterialTheme.colorScheme.errorContainer,
            contentColor = MaterialTheme.colorScheme.onErrorContainer,
        ) {
            Icon(
                Icons.Rounded.ErrorOutline,
                contentDescription = null,
                modifier = Modifier.padding(18.dp).size(32.dp),
            )
        }
        Text("承認できません", style = MaterialTheme.typography.titleLarge)
        Text(
            message,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
            modifier =
                Modifier.semantics {
                    liveRegion = LiveRegionMode.Assertive
                },
        )
        Button(onClick = onClose, modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp)) {
            Text("閉じる")
        }
    }
}

@Composable
private fun ErrorBanner(message: String, modifier: Modifier = Modifier) {
    Surface(
        shape = MaterialTheme.shapes.medium,
        color = MaterialTheme.colorScheme.errorContainer,
        contentColor = MaterialTheme.colorScheme.onErrorContainer,
        modifier =
            modifier.fillMaxWidth().semantics {
                liveRegion = LiveRegionMode.Assertive
            },
    ) {
        Row(
            modifier = Modifier.padding(12.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Icon(
                Icons.Rounded.ErrorOutline,
                contentDescription = null,
                modifier = Modifier.size(20.dp),
            )
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text("操作を完了できませんでした", style = MaterialTheme.typography.titleSmall)
                Text(message, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

@Composable
private fun LoadingPanel() {
    Box(Modifier.fillMaxWidth().padding(48.dp), contentAlignment = Alignment.Center) {
        CircularProgressIndicator()
    }
}
