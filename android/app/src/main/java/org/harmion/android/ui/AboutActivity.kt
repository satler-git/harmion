package org.harmion.android.ui

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.OpenInNew
import androidx.compose.material.icons.rounded.Security
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.fragment.app.FragmentActivity
import java.time.OffsetDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import org.harmion.android.BuildConfig
import org.harmion.android.ui.theme.HarmionTheme

/** Version, build revision and project links. */
class AboutActivity : FragmentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            HarmionTheme {
                AboutScreen(onBack = { finish() })
            }
        }
    }
}

private const val REPOSITORY = "https://github.com/satler-git/harmion"

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AboutScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    val open = { url: String ->
        runCatching {
            context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
        }
        Unit
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("このアプリについて") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = "戻る")
                    }
                },
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier.fillMaxWidth().padding(padding),
            contentPadding = PaddingValues(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            item {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    Surface(
                        shape = MaterialTheme.shapes.extraLarge,
                        color = MaterialTheme.colorScheme.primaryContainer,
                        contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
                    ) {
                        Icon(
                            Icons.Rounded.Security,
                            contentDescription = null,
                            modifier = Modifier.padding(18.dp).size(36.dp),
                        )
                    }
                    Column {
                        Text(
                            "Harmion",
                            style = MaterialTheme.typography.headlineMedium,
                            fontWeight = FontWeight.Bold,
                        )
                    }
                }
            }

            item {
                HarmionCard {
                    SectionHeader("バージョン")
                    InfoRow("アプリ", BuildConfig.VERSION_NAME)
                    InfoRow("ビルド", if (BuildConfig.OFFICIAL_RELEASE) "公式リリース" else "開発ビルド")
                    InfoRow("種別", BuildConfig.BUILD_TYPE)
                    InfoRow("ビルド日時", formatBuildTime(BuildConfig.BUILD_TIME))
                }
            }

            item {
                HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                    SectionHeader("リビジョン")
                    InfoRow(
                        "コミット",
                        BuildConfig.GIT_REVISION +
                            if (BuildConfig.GIT_DIRTY) " (変更あり)" else "",
                        emphasis = true,
                    )
                    InfoRow("ブランチ", BuildConfig.GIT_BRANCH)
                    InfoRow("パッケージ", BuildConfig.APPLICATION_ID)
                }
            }

            item {
                HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                    OutlinedButton(
                        onClick = { context.startActivity(Intent(context, LicensesActivity::class.java)) },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text("ライセンス")
                    }
                    OutlinedButton(onClick = { open(REPOSITORY) }, modifier = Modifier.fillMaxWidth()) {
                        Icon(Icons.AutoMirrored.Rounded.OpenInNew, contentDescription = null, modifier = Modifier.size(18.dp))
                        Text("GitHub", modifier = Modifier.padding(start = 8.dp))
                    }
                }
            }

            item {
                Text(
                    "MIT License · © 2026 satler",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

private fun formatBuildTime(raw: String): String =
    runCatching {
        OffsetDateTime.parse(raw)
            .atZoneSameInstant(ZoneId.systemDefault())
            .format(DateTimeFormatter.ofPattern("yyyy/MM/dd HH:mm"))
    }.getOrDefault(raw)
