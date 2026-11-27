package org.harmion.mote

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import com.mikepenz.aboutlibraries.ui.compose.android.produceLibraries
import com.mikepenz.aboutlibraries.ui.compose.m3.LibrariesContainer
import com.mikepenz.aboutlibraries.ui.compose.variant.LibraryDetailMode

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun MoteLicensesScreen(onBack: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val libraries by produceLibraries(R.raw.aboutlibraries)
    BackHandler(onBack = onBack)
    Scaffold(
        containerColor = colors.background,
        topBar = {
            TopAppBar(
                title = { Text("ライセンス") },
                navigationIcon = { IconButton(onClick = onBack) { MoteIcon(MoteGlyph.Back, "戻る") } },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = colors.background),
            )
        },
    ) { insets ->
        LibrariesContainer(
            libraries = libraries,
            modifier = Modifier.fillMaxSize().padding(insets),
            detailMode = LibraryDetailMode.Dialog,
            licenseDialogConfirmText = "閉じる",
        )
    }
}
