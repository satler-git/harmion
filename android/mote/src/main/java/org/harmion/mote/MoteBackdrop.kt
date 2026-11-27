package org.harmion.mote

import android.annotation.SuppressLint
import android.os.Build
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.BlurEffect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.layer.GraphicsLayer
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.graphics.rememberGraphicsLayer
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.toIntSize
import kotlin.math.ceil

/**
 * Content recorded by the scrolling surface so the header and dock can draw a
 * blurred copy of what is behind them. Android 12+ only; older versions fall
 * back to a translucent scrim.
 */
internal class MoteBackdrop internal constructor(
    val content: GraphicsLayer,
    val blur: GraphicsLayer,
) {
    val blurSupported: Boolean get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
}

@Composable
internal fun rememberMoteBackdrop(): MoteBackdrop {
    val content = rememberGraphicsLayer()
    val blur = rememberGraphicsLayer()
    return remember(content, blur) { MoteBackdrop(content, blur) }
}

/** Draws the content normally while recording it for the bars. */
internal fun Modifier.moteBackdropSource(backdrop: MoteBackdrop): Modifier =
    drawWithContent {
        backdrop.content.record(size.toIntSize()) { this@drawWithContent.drawContent() }
        drawLayer(backdrop.content)
    }

/**
 * A bar that shows the blurred content behind it. [tint] is layered over the
 * blur so icon/text contrast does not depend on the message underneath.
 */
@Composable
internal fun BlurredBar(
    backdrop: MoteBackdrop,
    modifier: Modifier = Modifier,
    tint: Color,
    content: @Composable BoxScope.() -> Unit,
) {
    var position by remember { mutableStateOf(Offset.Zero) }
    val support = backdrop.blurSupported
    Box(
        modifier = modifier
            .clipToBounds()
            .onGloballyPositioned { position = it.positionInRoot() }
            .then(
                if (support) {
                    Modifier.drawBarBlur(backdrop, position, tint)
                } else {
                    Modifier.drawScrim(tint)
                },
            ),
        content = content,
    )
}

// BlurEffect is API 31+; BlurredBar chooses this path only on supported devices.
@SuppressLint("NewApi")
private fun Modifier.drawBarBlur(backdrop: MoteBackdrop, position: Offset, tint: Color): Modifier =
    drawBehind {
        val radius = 22.dp.toPx()
        val margin = ceil(radius * 1.5f).toInt()
        val bar = size.toIntSize()
        backdrop.blur.renderEffect = BlurEffect(radius, radius, TileMode.Clamp)
        backdrop.blur.record(
            density = this,
            layoutDirection = layoutDirection,
            size = IntSize(bar.width, bar.height + margin * 2),
        ) {
            drawRect(tint.copy(alpha = 1f))
            translate(-position.x, margin - position.y) { drawLayer(backdrop.content) }
        }
        translate(0f, -margin.toFloat()) { drawLayer(backdrop.blur) }
        drawRect(
            Brush.verticalGradient(listOf(tint, tint.copy(alpha = tint.alpha * .55f))),
        )
    }

private fun Modifier.drawScrim(tint: Color): Modifier =
    drawBehind {
        drawRect(Brush.verticalGradient(listOf(tint, tint.copy(alpha = tint.alpha * .55f))))
    }
