package org.harmion.mote.voice

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.harmion.mote.MoteTheme

class VoiceProbeActivity : ComponentActivity() {
    private var permissionMessage by mutableStateOf<String?>(null)
    private val permissions = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        permissionMessage = if (it.values.all { granted -> granted }) {
            "権限を確認しました。開始を押すと録音します。"
        } else {
            "マイクと録音通知の権限が必要です。"
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            val state by VoiceProbeService.state.collectAsStateWithLifecycle()
            MoteTheme {
                Surface(Modifier.fillMaxSize()) {
                    Column(
                        Modifier.safeDrawingPadding().padding(24.dp),
                        verticalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Text("Mote 録音検証", style = MaterialTheme.typography.headlineSmall)
                        Text("音声を端末内に WAV で保持します。画面を離れた後は通知から停止できます。")
                        Text(state.message)
                        state.recordingId?.let { Text("録音 ID  $it") }
                        Text("保存済み  ${state.savedSamples / PROBE_SAMPLE_RATE} 秒")
                        permissionMessage?.let { Text(it) }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Button(onClick = ::startRecording, enabled = !state.active) { Text("開始") }
                            Button(
                                onClick = { state.recordingId?.let { VoiceProbeService.stop(this@VoiceProbeActivity, it) } },
                                enabled = state.active && !state.stopping,
                            ) { Text("停止") }
                        }
                    }
                }
            }
        }
    }

    private fun startRecording() {
        val missing = buildList {
            add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        }.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty()) {
            permissions.launch(missing.toTypedArray())
            return
        }
        if (!lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) return
        permissionMessage = null
        try {
            if (!VoiceProbeService.start(this)) permissionMessage = "録音検証の通知を有効にしてください。"
        } catch (_: IllegalStateException) {
            permissionMessage = "録音を開始できませんでした。画面を開いたまま再試行してください。"
        } catch (_: SecurityException) {
            permissionMessage = "録音を開始する権限がありません。"
        }
    }
}
