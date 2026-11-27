package org.harmion.mote

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun MoteComposer(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    readOnly: Boolean = false,
    maxLines: Int = 5,
    focusRequester: FocusRequester? = null,
    translucent: Boolean = false,
    actionCount: Int = 1,
    actions: @Composable RowScope.() -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val interactions = remember { MutableInteractionSource() }
    val focused by interactions.collectIsFocusedAsState()
    val focusManager = LocalFocusManager.current
    val imeVisible = WindowInsets.isImeVisible
    var wasImeVisible by remember { mutableStateOf(false) }
    // A dismissed IME must not leave a blinking cursor on an unfocused-looking field.
    LaunchedEffect(imeVisible) {
        if (wasImeVisible && !imeVisible) focusManager.clearFocus()
        wasImeVisible = imeVisible
    }
    val stroke by animateColorAsState(
        if (focused) colors.primary.copy(alpha = .65f) else colors.outlineVariant,
        MaterialTheme.motionScheme.fastEffectsSpec(),
        label = "composer focus",
    )
    val shape = RoundedCornerShape(25.dp)
    val fieldShape = RoundedCornerShape(19.dp)
    val container = if (translucent) colors.surfaceContainer.copy(alpha = .86f) else colors.surfaceContainer
    val fieldColor = if (translucent) colors.surfaceContainerHigh.copy(alpha = .90f) else colors.surfaceContainerHigh
    val actionPadding = when {
        actionCount <= 0 -> 0.dp
        else -> 48.dp * actionCount + 2.dp * (actionCount - 1) + 17.dp
    }
    Column(
        modifier.fillMaxWidth().background(container, shape).border(1.dp, stroke, shape)
            .padding(5.dp)
            .animateContentSize(tween(durationMillis = 160), alignment = Alignment.BottomStart),
    ) {
        val textStyle = MaterialTheme.typography.bodyMedium
        Box(Modifier.fillMaxWidth()) {
            BasicTextField(
                value = value,
                onValueChange = onValueChange,
                modifier = Modifier.fillMaxWidth()
                    .padding(end = actionPadding)
                    .then(if (focusRequester != null) Modifier.focusRequester(focusRequester) else Modifier)
                    .background(fieldColor, fieldShape)
                    .semantics { contentDescription = placeholder },
                enabled = enabled,
                readOnly = readOnly,
                textStyle = textStyle.copy(color = colors.onSurface),
                maxLines = maxLines,
                interactionSource = interactions,
                cursorBrush = SolidColor(colors.primary),
                decorationBox = { inner ->
                    Box(
                        Modifier.fillMaxWidth().heightIn(min = 48.dp)
                            .padding(horizontal = 12.dp, vertical = 10.dp),
                        contentAlignment = Alignment.CenterStart,
                    ) {
                        if (value.isEmpty()) Text(placeholder, color = colors.onSurfaceVariant, style = textStyle)
                        inner()
                    }
                },
            )
            Row(
                Modifier.align(Alignment.BottomEnd).padding(end = 5.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                ComposerActions(actions)
            }
        }
    }
}


@Composable
private fun ComposerActions(actions: @Composable RowScope.() -> Unit) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) { actions() }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun MoteSendButton(
    enabled: Boolean,
    label: String,
    onClick: () -> Unit,
    glyph: MoteGlyph = MoteGlyph.Send,
) {
    val colors = MaterialTheme.colorScheme
    FilledIconButton(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.size(48.dp),
        colors = IconButtonDefaults.filledIconButtonColors(
            containerColor = colors.primary,
            contentColor = colors.onPrimary,
            disabledContainerColor = colors.surfaceContainerHighest,
            disabledContentColor = colors.onSurfaceVariant.copy(alpha = .72f),
        ),
    ) { MoteIcon(glyph, label, Modifier.size(21.dp)) }
}
