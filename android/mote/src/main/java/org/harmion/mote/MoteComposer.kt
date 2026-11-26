package org.harmion.mote

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
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
    actions: @Composable RowScope.() -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val interactions = remember { MutableInteractionSource() }
    val focused by interactions.collectIsFocusedAsState()
    val stroke by animateColorAsState(
        if (focused) colors.primary.copy(alpha = .65f) else colors.outlineVariant,
        MaterialTheme.motionScheme.fastEffectsSpec(),
        label = "composer focus",
    )
    val shape = RoundedCornerShape(25.dp)
    val fieldShape = RoundedCornerShape(19.dp)
    Row(
        modifier.fillMaxWidth().background(colors.surfaceContainer, shape).border(1.dp, stroke, shape)
            .padding(5.dp),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        val textStyle = MaterialTheme.typography.bodyMedium
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            modifier = Modifier.weight(1f).defaultMinSize(minHeight = 48.dp)
                .background(colors.surfaceContainerHigh, fieldShape)
                .semantics { contentDescription = placeholder },
            enabled = enabled,
            readOnly = readOnly,
            textStyle = textStyle.copy(color = colors.onSurface),
            maxLines = maxLines,
            interactionSource = interactions,
            cursorBrush = SolidColor(colors.primary),
            decorationBox = { field ->
                Box(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp), contentAlignment = Alignment.CenterStart) {
                    if (value.isEmpty()) Text(placeholder, color = colors.onSurfaceVariant, style = textStyle)
                    field()
                }
            },
        )
        actions()
    }
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
        shapes = IconButtonDefaults.shapes(shape = CircleShape, pressedShape = RoundedCornerShape(14.dp)),
        colors = IconButtonDefaults.filledIconButtonColors(
            containerColor = colors.primary,
            contentColor = colors.onPrimary,
            disabledContainerColor = colors.surfaceContainerHigh,
            disabledContentColor = colors.onSurfaceVariant.copy(alpha = .72f),
        ),
    ) { MoteIcon(glyph, label, Modifier.size(21.dp)) }
}
