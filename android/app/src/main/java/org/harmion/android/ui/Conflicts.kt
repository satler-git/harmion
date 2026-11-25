package org.harmion.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.MergeType
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.harmion.android.core.NodeRuntimeController
import org.json.JSONArray
import org.json.JSONObject

/** Loads and resolves field conflicts. Renders nothing when there are none. */
@Composable
fun ConflictsSection(controller: NodeRuntimeController) {
    var conflicts by remember { mutableStateOf<List<JSONObject>>(emptyList()) }
    var open by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(Unit) {
        conflicts =
            runCatching {
                withContext(Dispatchers.IO) { loadConflicts(controller) }
            }.onFailure { error = it.message }.getOrDefault(emptyList())
    }

    if (conflicts.isEmpty()) return

    HarmionCard(containerColor = MaterialTheme.colorScheme.errorContainer) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Icon(Icons.Rounded.Warning, contentDescription = null)
            Column(Modifier.weight(1f)) {
                Text("競合 ${conflicts.size} 件", style = MaterialTheme.typography.titleSmall)
                error?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            }
            Button(onClick = { open = true }) { Text("確認") }
        }
    }

    if (open) {
        ConflictSheet(
            controller = controller,
            conflicts = conflicts,
            onDismiss = { open = false },
            onResolved = {
                open = false
                conflicts = emptyList()
            },
        )
    }
}

private suspend fun loadConflicts(controller: NodeRuntimeController): List<JSONObject> {
    val admin = controller.adminClient()
    try {
        val page = admin.query("""{"type":"and","conditions":[]}""", 32)
        val items = page.optJSONArray("items") ?: return emptyList()
        val result = mutableListOf<JSONObject>()
        for (index in 0 until items.length()) {
            val item = items.getJSONObject(index)
            val conflicted =
                item.optBoolean("deletion_conflict") ||
                    (item.optJSONArray("conflicts")?.length() ?: 0) > 0
            if (conflicted) result += item
        }
        return result
    } finally {
        admin.close()
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ConflictSheet(
    controller: NodeRuntimeController,
    conflicts: List<JSONObject>,
    onDismiss: () -> Unit,
    onResolved: () -> Unit,
) {
    var selected by remember { mutableStateOf<JSONObject?>(null) }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        if (selected == null) {
            LazyColumn(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                item {
                    Text("競合", style = MaterialTheme.typography.headlineSmall)
                }
                items(conflicts, key = { it.optJSONObject("content")?.optString("id").orEmpty() }) { item ->
                    val content = item.optJSONObject("content")
                    val id = content?.optString("id").orEmpty()
                    val body = content?.optString("body").orEmpty()
                    HarmionCard(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                        Text(
                            body.ifBlank { shortId(id) },
                            style = MaterialTheme.typography.bodyMedium,
                            maxLines = 2,
                        )
                        OutlinedButton(
                            onClick = { selected = item },
                            modifier = Modifier.fillMaxWidth(),
                        ) { Text("解決") }
                    }
                }
                item { androidx.compose.foundation.layout.Spacer(Modifier.size(24.dp)) }
            }
        } else {
            ConflictDetail(
                controller = controller,
                item = selected!!,
                onBack = { selected = null },
                onResolved = onResolved,
            )
        }
    }
}

@Composable
private fun ConflictDetail(
    controller: NodeRuntimeController,
    item: JSONObject,
    onBack: () -> Unit,
    onResolved: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    val contentId = item.optJSONObject("content")?.optString("id").orEmpty()
    var detail by remember(contentId) { mutableStateOf<JSONObject?>(null) }
    var chosen by remember(contentId) { mutableStateOf<Map<String, JSONObject>>(emptyMap()) }
    var merging by remember(contentId) { mutableStateOf<String?>(null) }
    var error by remember(contentId) { mutableStateOf<String?>(null) }
    var busy by remember(contentId) { mutableStateOf(false) }

    LaunchedEffect(contentId) {
        detail =
            runCatching {
                withContext(Dispatchers.IO) {
                    val admin = controller.adminClient()
                    try {
                        admin.conflicts(contentId)
                    } finally {
                        admin.close()
                    }
                }
            }.onFailure { error = it.message }.getOrNull()
    }

    val groups = detail?.optJSONArray("candidates")

    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = onBack) { Text("戻る") }
            Text("競合を解決", style = MaterialTheme.typography.titleLarge)
        }
        if (detail == null) {
            if (error == null) CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
        } else if (groups == null || groups.length() == 0) {
            Text("候補がありません", style = MaterialTheme.typography.bodyMedium)
        } else {
            for (index in 0 until groups.length()) {
                val group = groups.getJSONObject(index)
                val field = group.getJSONObject("field")
                val key = field.toString()
                Text(
                    fieldLabel(field),
                    style = MaterialTheme.typography.titleSmall,
                    modifier = Modifier.padding(top = 8.dp),
                )
                val candidates = group.getJSONArray("candidates")
                for (candidateIndex in 0 until candidates.length()) {
                    val candidate = candidates.getJSONObject(candidateIndex)
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RadioButton(
                            selected = chosen[key] == candidate,
                            onClick = { chosen = chosen + (key to candidate) },
                        )
                        Text(
                            valueText(candidate.getJSONObject("value")),
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier.weight(1f),
                            fontFamily =
                                if (field.optString("type") == "body") {
                                    FontFamily.Monospace
                                } else {
                                    FontFamily.Default
                                },
                        )
                    }
                }
                if (field.optString("type") == "body" && candidates.length() >= 2) {
                    OutlinedButton(
                        onClick = {
                            val versions = mutableListOf<String>()
                            for (c in 0 until candidates.length()) {
                                val list = candidates.getJSONObject(c).getJSONArray("versions")
                                for (v in 0 until list.length()) versions += list.getString(v)
                            }
                            if (versions.size < 2) return@OutlinedButton
                            scope.launch {
                                busy = true
                                merging = null
                                runCatching {
                                    withContext(Dispatchers.IO) {
                                        val admin = controller.adminClient()
                                        try {
                                            val token =
                                                detail!!.getJSONObject("view").getString("edit_token")
                                            fun plan(base: String) =
                                                admin.mergePlan(
                                                    contentId,
                                                    token,
                                                    versions[0],
                                                    versions[1],
                                                    base,
                                                )
                                            var result = plan("")
                                            if (result.optString("status") == "choose_base") {
                                                val bases = result.optJSONArray("versions")
                                                val base =
                                                    bases?.optString(0).orEmpty().ifEmpty {
                                                        versions.first()
                                                    }
                                                result = plan(base)
                                            }
                                            result
                                        } finally {
                                            admin.close()
                                        }
                                    }
                                }.onSuccess { result ->
                                    if (result.optString("status") == "proposed") {
                                        merging = result.optString("text")
                                    } else {
                                        error = "マージの基準を決められませんでした"
                                    }
                                }.onFailure { error = it.message }
                                busy = false
                            }
                        },
                        enabled = !busy,
                        modifier = Modifier.padding(start = 40.dp),
                    ) {
                        Icon(Icons.Rounded.MergeType, contentDescription = null, modifier = Modifier.size(16.dp))
                        Text("3-wayマージ", modifier = Modifier.padding(start = 6.dp))
                    }
                }
            }

            merging?.let { text ->
                Text(
                    "マージ結果",
                    style = MaterialTheme.typography.labelLarge,
                    modifier = Modifier.padding(top = 12.dp),
                )
                Text(
                    text,
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                    modifier = Modifier.padding(vertical = 6.dp),
                )
                Button(
                    onClick = {
                        scope.launch {
                            busy = true
                            runCatching {
                                withContext(Dispatchers.IO) {
                                    val admin = controller.adminClient()
                                    try {
                                        val token =
                                            detail!!.getJSONObject("view").getString("edit_token")
                                        val bodyGroup =
                                            (0 until groups.length()).map { groups.getJSONObject(it) }
                                                .first { it.getJSONObject("field").optString("type") == "body" }
                                        val replaces = JSONArray()
                                        val candidates = bodyGroup.getJSONArray("candidates")
                                        for (c in 0 until candidates.length()) {
                                            val list = candidates.getJSONObject(c).getJSONArray("versions")
                                            for (v in 0 until list.length()) replaces.put(list.getString(v))
                                        }
                                        admin.resolve(
                                            contentId,
                                            token,
                                            JSONArray().put(
                                                JSONObject()
                                                    .put("field", fieldSpec(bodyGroup.getJSONObject("field")))
                                                    .put("text", text)
                                                    .put("replaces", replaces),
                                            ),
                                        )
                                    } finally {
                                        admin.close()
                                    }
                                }
                            }.onSuccess { onResolved() }
                                .onFailure { error = it.message }
                            busy = false
                        }
                    },
                    enabled = !busy,
                    modifier = Modifier.fillMaxWidth(),
                ) { Text("この内容で解決") }
            }
        }
        error?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        }
        Button(
            onClick = {
                scope.launch {
                    busy = true
                    runCatching {
                        withContext(Dispatchers.IO) {
                            val admin = controller.adminClient()
                            try {
                                if (groups == null) return@withContext
                                val selections = JSONArray()
                                val token = detail!!.getJSONObject("view").getString("edit_token")
                                for (index in 0 until groups.length()) {
                                    val group = groups.getJSONObject(index)
                                    val key = group.getJSONObject("field").toString()
                                    val candidate = chosen[key] ?: continue
                                    val replaces = JSONArray()
                                    val candidates = group.getJSONArray("candidates")
                                    for (c in 0 until candidates.length()) {
                                        val list = candidates.getJSONObject(c).getJSONArray("versions")
                                        for (v in 0 until list.length()) replaces.put(list.getString(v))
                                    }
                                    selections.put(
                                        JSONObject()
                                            .put("field", fieldSpec(group.getJSONObject("field")))
                                            .put(
                                                "version",
                                                candidate.getJSONArray("versions").getString(0),
                                            )
                                            .put("replaces", replaces),
                                    )
                                }
                                require(selections.length() > 0) { "項目を選択してください" }
                                admin.select(contentId, token, selections)
                            } finally {
                                admin.close()
                            }
                        }
                    }.onSuccess { onResolved() }
                        .onFailure { error = it.message }
                    busy = false
                }
            },
            enabled = !busy && chosen.isNotEmpty(),
            modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
        ) { Text("選択した内容で解決") }
        androidx.compose.foundation.layout.Spacer(Modifier.size(24.dp))
    }
}

private fun fieldSpec(field: JSONObject): Any =
    when (field.optString("type")) {
        "body" -> "body"
        "attribute" -> JSONObject().put("field", "attribute").put("namespace", field.optString("namespace")).put("key", field.optString("key"))
        "kind" -> JSONObject().put("field", "kind").put("name", field.optString("name"))
        else -> error("unknown field")
    }

private fun fieldLabel(field: JSONObject): String =
    when (field.optString("type")) {
        "body" -> "本文"
        "attribute" -> "${field.optString("namespace")}/${field.optString("key")}"
        "kind" -> "kind: ${field.optString("name")}"
        else -> "項目"
    }

private fun valueText(value: JSONObject): String =
    when (value.optString("type")) {
        "body" -> value.optString("value").replace("\n", " ").take(200)
        "absent" -> "（なし）"
        "kind" -> if (value.optBoolean("value")) "あり" else "なし"
        "attribute" -> value.opt("value")?.toString()?.take(200) ?: "（なし）"
        else -> "—"
    }
