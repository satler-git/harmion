package org.harmion.android.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Apps
import androidx.compose.material.icons.rounded.Group
import androidx.compose.material.icons.rounded.Home
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.LargeTopAppBar
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.fragment.app.FragmentActivity
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.MutableStateFlow
import org.harmion.android.HarmionApplication
import org.harmion.android.core.NodeRuntimeController
import org.harmion.android.core.PolicyEvaluator
import org.harmion.android.core.RuntimeSettings
import org.harmion.android.core.RuntimeSnapshot
import androidx.glance.appwidget.updateAll
import org.harmion.android.widget.HarmionWidget
import org.harmion.android.ui.theme.HarmionTheme

class MainActivity : FragmentActivity() {
    private val scanned = MutableStateFlow<String?>(null)
    private val syncRequests = MutableStateFlow(0)

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        if (intent.getBooleanExtra("sync", false)) syncRequests.value += 1
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (intent?.getBooleanExtra("sync", false) == true) syncRequests.value += 1
        val scanLauncher =
            registerForActivityResult(ScanContract()) { result ->
                scanned.value = result?.contents
            }
        val cameraPermission =
            registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
                if (granted) {
                    scanLauncher.launch(scanOptions())
                }
            }
        val notificationPermission =
            registerForActivityResult(ActivityResultContracts.RequestPermission()) {}
        val requestScan = {
            if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
                scanLauncher.launch(scanOptions())
            } else {
                cameraPermission.launch(Manifest.permission.CAMERA)
            }
            true
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        setContent {
            HarmionTheme {
                val application = LocalContext.current.applicationContext as HarmionApplication
                val controller = application.runtime
                val policy = application.policy
                var initialized by remember { mutableStateOf(controller.isInitialized) }
                if (!initialized) {
                    OnboardingScreen(
                        controller = controller,
                        scanned = scanned,
                        onScan = requestScan,
                        onInitialized = { initialized = true },
                    )
                } else {
                    val snapshot by controller.snapshot.collectAsStateWithLifecycle()
                    MainScaffold(
                        controller = controller,
                        policy = policy,
                        snapshot = snapshot,
                        syncRequests = syncRequests,
                    )
                }
            }
        }
    }

    private fun scanOptions(): ScanOptions =
        ScanOptions()
            .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
            .setPrompt("Harmionの招待コードを読み取ります")
            .setBeepEnabled(false)
            .setOrientationLocked(false)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MainScaffold(
    controller: NodeRuntimeController,
    policy: PolicyEvaluator,
    snapshot: RuntimeSnapshot?,
    syncRequests: MutableStateFlow<Int>,
) {
    var tab by rememberSaveable { mutableIntStateOf(0) }
    val lifecycleOwner = LocalLifecycleOwner.current
    val scope = rememberCoroutineScope()

    // Keep the runtime only while the screen is actually started. Releasing on
    // composition disposal alone would keep the node alive after HOME because a
    // stopped activity keeps its composition.
    DisposableEffect(lifecycleOwner, controller) {
        var acquired = false
        val observer =
            LifecycleEventObserver { _, event ->
                when (event) {
                    Lifecycle.Event.ON_START -> {
                        if (!acquired) {
                            acquired = true
                            controller.acquire()
                            scope.launch {
                                controller.refreshPolicy()
                                controller.ensureStarted()
                            }
                        }
                    }
                    Lifecycle.Event.ON_STOP,
                    Lifecycle.Event.ON_DESTROY -> {
                        if (acquired) {
                            acquired = false
                            controller.release()
                        }
                    }
                    else -> Unit
                }
            }
        lifecycleOwner.lifecycle.addObserver(observer)
        if (lifecycleOwner.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) {
            acquired = true
            controller.acquire()
            scope.launch {
                controller.refreshPolicy()
                controller.ensureStarted()
            }
        }
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            if (acquired) controller.release()
        }
    }
    val context = androidx.compose.ui.platform.LocalContext.current
    val syncCount by syncRequests.collectAsStateWithLifecycle()
    LaunchedEffect(syncCount) {
        if (syncCount > 0) runCatching { controller.syncNow() }
    }
    LaunchedEffect(snapshot) {
        val running = snapshot?.running == true
        val status =
            when {
                !running -> "停止中"
                snapshot?.syncBlockedCode != null -> "同期待機"
                !snapshot?.inflight.isNullOrEmpty() -> "同期中"
                else -> "稼働中"
            }
        RuntimeSettings.writeWidgetState(context, status, snapshot?.peers ?: 0)
        runCatching { HarmionWidget().updateAll(context) }
    }

    val tabs =
        listOf(
            Dest(0, "ホーム", "ノードと同期の状態", Icons.Rounded.Home),
            Dest(1, "ピア", "端末をつなぐ", Icons.Rounded.Group),
            Dest(2, "アプリ", "接続と権限", Icons.Rounded.Apps),
            Dest(3, "設定", "同期と所有者", Icons.Rounded.Settings),
        )
    val current = tabs[tab]
    val scrollBehavior = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()

    Scaffold(
        modifier = Modifier.fillMaxSize(),
        topBar = {
            LargeTopAppBar(
                title = {
                    Column {
                        Text(current.title, style = MaterialTheme.typography.headlineMedium)
                        Text(
                            current.subtitle,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                },
                scrollBehavior = scrollBehavior,
            )
        },
        bottomBar = {
            NavigationBar {
                tabs.forEach { item ->
                    NavigationBarItem(
                        selected = tab == item.index,
                        onClick = { tab = item.index },
                        icon = { Icon(item.icon, contentDescription = item.title) },
                        label = { Text(item.title) },
                    )
                }
            }
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection)) {
            when (tab) {
                0 -> HomeScreen(controller, snapshot)
                1 -> PeersScreen(controller)
                2 -> AppsScreen(controller)
                else -> SettingsScreen(controller, policy)
            }
        }
    }
}

private data class Dest(
    val index: Int,
    val title: String,
    val subtitle: String,
    val icon: androidx.compose.ui.graphics.vector.ImageVector,
)
