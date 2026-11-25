package org.harmion.android.widget

import android.content.Context
import android.content.Intent
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.LocalContext
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.background
import androidx.glance.layout.Alignment
import androidx.glance.layout.Box
import androidx.glance.layout.Column
import androidx.glance.layout.Row
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.padding
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.glance.color.ColorProvider as dayNightColor
import org.harmion.android.core.RuntimeSettings
import org.harmion.android.ui.MainActivity

class HarmionWidget : GlanceAppWidget() {
    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val state = RuntimeSettings.widgetState(context)
        provideContent { Content(state.status, state.peers) }
    }
}

@Composable
private fun Content(status: String, peers: Int) {
    val context = LocalContext.current
    Column(
        modifier =
            GlanceModifier.fillMaxSize()
                .background(dayNightColor(Color(0xFFE8EDFB), Color(0xFF1B2437)))
                .cornerRadius(24.dp)
                .padding(16.dp),
        verticalAlignment = Alignment.Vertical.CenterVertically,
    ) {
        Text(
            "Harmion",
            style =
                TextStyle(
                    color = dayNightColor(Color(0xFF33477A), Color(0xFFD7E2FF)),
                    fontSize = 13.sp,
                    fontWeight = FontWeight.Medium,
                ),
        )
        Text(
            status,
            style =
                TextStyle(
                    color = dayNightColor(Color(0xFF13203D), Color(0xFFF0F3FF)),
                    fontSize = 22.sp,
                    fontWeight = FontWeight.Bold,
                ),
        )
        Text(
            "$peers ピア",
            style =
                TextStyle(
                    color = dayNightColor(Color(0xFF44506B), Color(0xFFC3CAD9)),
                    fontSize = 13.sp,
                ),
        )
        Row(modifier = GlanceModifier.padding(top = 12.dp)) {
            Box(
                modifier =
                    GlanceModifier.background(dayNightColor(Color(0xFF4A5F97), Color(0xFFAFC4FF)))
                        .cornerRadius(14.dp)
                        .padding(horizontal = 14.dp, vertical = 8.dp)
                        .clickable(actionStartActivity(Intent(context, MainActivity::class.java))),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    "開く",
                    style =
                        TextStyle(
                            color = dayNightColor(Color.White, Color(0xFF13203D)),
                            fontSize = 13.sp,
                            fontWeight = FontWeight.Medium,
                        ),
                )
            }
            Box(
                modifier =
                    GlanceModifier.padding(start = 8.dp)
                        .background(dayNightColor(Color(0x33FFFFFF), Color(0x22000000)))
                        .cornerRadius(14.dp)
                        .padding(horizontal = 14.dp, vertical = 8.dp)
                        .clickable(
                            actionStartActivity(
                                Intent(context, MainActivity::class.java).putExtra("sync", true)
                            )
                        ),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    "同期",
                    style =
                        TextStyle(
                            color = dayNightColor(Color(0xFF33477A), Color(0xFFD7E2FF)),
                            fontSize = 13.sp,
                        ),
                )
            }
        }
    }
}
