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
        val unauthorized =
            Button(this).apply {
                text = "未認可で status を呼ぶ"
                setOnClickListener { runUnauthorizedStatus() }
            }
        root.addView(title, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        root.addView(status, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        root.addView(connect, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        root.addView(verify, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        root.addView(unauthorized, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
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

    private fun runUnauthorizedStatus() {
        status.text = "未認可 status を呼び出し中…"
        lifecycleScope.launch {
            var session: HarmionSession? = null
            try {
                session = Harmion.connect(this@MainActivity)
                val nodeStatus = session.status()
                status.text = "status OK: running=${nodeStatus.running} membership=${nodeStatus.membership}"
            } catch (error: Exception) {
                status.text = "拒否されました: ${error.message}"
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
                val nodeStatus = session.status()
                val saved = session.createContent("verify ${System.currentTimeMillis()}")
                status.text =
                    "status OK: running=${nodeStatus.running} membership=${nodeStatus.membership}\n" +
                        "保存OK ${saved.updateId}"
            } catch (error: Exception) {
                status.text = "失敗: ${error.message}"
            } finally {
                session?.close()
            }
        }
    }
}
