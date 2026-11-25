package org.harmion.testclient

import android.graphics.Color
import android.os.Bundle
import android.view.Gravity
import android.view.ViewGroup
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import org.harmion.sdk.AuthorizationRequest
import org.harmion.sdk.Filter
import org.harmion.sdk.Harmion
import org.harmion.sdk.HarmionSession

/** Debug-only client used to exercise the SDK approval flow on a device. */
class MainActivity : ComponentActivity() {
    private lateinit var status: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root =
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(48, 64, 48, 32)
            }
        val title =
            TextView(this).apply {
                text = "Harmion E2E client"
                textSize = 22f
            }
        status =
            TextView(this).apply {
                text = "接続していません"
                textSize = 15f
                setTextColor(Color.DKGRAY)
                setPadding(0, 32, 0, 32)
            }
        val connect =
            Button(this).apply {
                text = "接続を要求（read/write, 90日）"
                setOnClickListener { runConnect() }
            }
        val verify =
            Button(this).apply {
                text = "既存の認可を確認して保存"
                setOnClickListener { runSave() }
            }
        root.addView(title, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        root.addView(status, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        root.addView(connect, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        root.addView(verify, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        setContentView(ScrollView(this).apply { addView(root) })
    }

    private fun runConnect() {
        status.text = "接続中…"
        lifecycleScope.launch {
            var session: HarmionSession? = null
            try {
                session = Harmion.connect(this@MainActivity)
                val authorization =
                    session.authorize(
                        AuthorizationRequest(read = Filter.All, write = Filter.All),
                        "Harmion E2E",
                    )
                status.text =
                    "接続完了\nclient=${authorization.clientId.take(12)}…\n" +
                        "cluster=${session.nodeSummary().optString("cluster_id").take(12)}…"
                val saved = session.createContent("e2e ${System.currentTimeMillis()}")
                status.append("\n保存: ${saved.updateId.take(12)}…")
            } catch (error: Exception) {
                status.text = "失敗: ${error.message}"
            } finally {
                session?.close()
            }
        }
    }

    private fun runSave() {
        status.text = "接続を確認中…"
        lifecycleScope.launch {
            var session: HarmionSession? = null
            try {
                session = Harmion.connect(this@MainActivity)
                if (!session.isAuthorized()) {
                    status.text = "未認可です。先に接続を要求してください。"
                    return@launch
                }
                val saved = session.createContent("verify ${System.currentTimeMillis()}")
                status.text = "保存OK\n${saved.updateId}"
            } catch (error: Exception) {
                status.text = "失敗: ${error.message}"
            } finally {
                session?.close()
            }
        }
    }
}
