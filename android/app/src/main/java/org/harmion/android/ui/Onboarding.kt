package org.harmion.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Security
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
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
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.harmion.android.core.NodeRuntimeController
import org.json.JSONObject

@Composable
fun OnboardingScreen(
    controller: NodeRuntimeController,
    scanned: MutableStateFlow<String?>,
    onScan: () -> Boolean,
    onInitialized: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(Unit) {
        scanned.collect { value ->
            if (value == null) return@collect
            busy = true
            error = null
            message = null
            runCatching { joinCluster(controller, value) }
                .onSuccess {
                    message = it
                    onInitialized()
                }
                .onFailure { error = it.message ?: "参加できませんでした" }
            scanned.value = null
            busy = false
        }
    }

    Scaffold { padding ->
        Column(
            modifier =
                Modifier.fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(padding)
                    .padding(horizontal = 24.dp, vertical = 24.dp),
            verticalArrangement = Arrangement.spacedBy(20.dp),
        ) {
            Spacer(Modifier.height(16.dp))
            Surface(
                shape = MaterialTheme.shapes.extraLarge,
                color = MaterialTheme.colorScheme.primaryContainer,
                contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
            ) {
                Icon(
                    Icons.Rounded.Security,
                    contentDescription = null,
                    modifier = Modifier.padding(20.dp).size(36.dp),
                )
            }
            Text(
                "Harmion",
                style = MaterialTheme.typography.displaySmall,
                fontWeight = FontWeight.Bold,
            )
            Text(
                "承認した端末とだけ同期します。",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            OnboardingChoice(
                title = "この端末だけで始める",
                body = null,
                action = {
                    Button(
                        onClick = {
                            busy = true
                            error = null
                            scope.launch {
                                runCatching { controller.initialize() }
                                    .onSuccess { onInitialized() }
                                    .onFailure { error = it.message ?: "初期化できませんでした" }
                                busy = false
                            }
                        },
                        enabled = !busy,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Icon(Icons.Rounded.Add, contentDescription = null, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.size(8.dp))
                        Text("作成する")
                    }
                },
            )

            OnboardingChoice(
                title = "既存のクラスタに参加",
                body = null,
                action = {
                    FilledTonalButton(onClick = { onScan() }, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
                        Icon(
                            Icons.Rounded.QrCodeScanner,
                            contentDescription = null,
                            modifier = Modifier.size(18.dp),
                        )
                        Spacer(Modifier.size(8.dp))
                        Text("QRコードを読み取る")
                    }
                },
            )

            if (busy) {
                RowBusy("処理しています…")
            }
            message?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            Spacer(Modifier.height(24.dp))
        }
    }
}

@Composable
private fun OnboardingChoice(
    title: String,
    body: String?,
    action: @Composable () -> Unit,
) {
    Surface(
        shape = MaterialTheme.shapes.large,
        color = MaterialTheme.colorScheme.surfaceContainer,
    ) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(title, style = MaterialTheme.typography.titleLarge)
            body?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(Modifier.height(4.dp))
            action()
        }
    }
}

@Composable
private fun RowBusy(label: String) {
    androidx.compose.foundation.layout.Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
        Text(label, style = MaterialTheme.typography.bodyMedium)
    }
}

private suspend fun joinCluster(controller: NodeRuntimeController, text: String): String =
    withContext(Dispatchers.IO) {
        val registration =
            PeerCode.decode(text)
                ?: throw IllegalArgumentException("Harmionの招待コードではありません")
        val json = JSONObject(String(registration, Charsets.UTF_8))
        val clusterId = json.optString("cluster_id")
        require(clusterId.length == 64) { "参加先クラスタを読み取れませんでした" }
        if (!controller.isInitialized) {
            controller.initialize(clusterId = clusterId, useIroh = true)
        }
        if (!controller.ensureStarted()) {
            throw IllegalStateException("ノードを開始できませんでした")
        }
        val admin = controller.adminClient()
        try {
            admin.join(registration)
        } finally {
            admin.close()
        }
        "参加要求を記録しました。既存ノードの所有者に承認を依頼してください。"
    }
