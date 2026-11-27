package org.harmion.mote

import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathBuilder
import androidx.compose.ui.graphics.vector.path
import androidx.compose.ui.unit.dp

internal enum class MoteGlyph {
    Back, More, Pin, Archive, Restore, Copy, Share, Delete, Expand, Send, Check, Close, Inbox, Edit, Link, Alert, Dots, Settings,
    Sync, Reply, Fold, Unfold, Info,
}

@Composable
internal fun MoteIcon(
    glyph: MoteGlyph,
    contentDescription: String?,
    modifier: Modifier = Modifier,
    tint: Color = LocalContentColor.current,
) {
    Icon(
        imageVector = MoteGlyphs.getValue(glyph),
        contentDescription = contentDescription,
        modifier = modifier,
        tint = tint,
    )
}

@Composable
internal fun MoteMark(modifier: Modifier = Modifier, color: Color = MaterialTheme.colorScheme.primary) {
    Icon(imageVector = ClusterMark, contentDescription = null, modifier = modifier, tint = color)
}

private val Ink = SolidColor(Color.Black)

private fun outline(
    glyph: MoteGlyph,
    autoMirror: Boolean = false,
    draw: PathBuilder.() -> Unit,
): ImageVector =
    ImageVector.Builder(glyph.name, 24.dp, 24.dp, 24f, 24f, autoMirror = autoMirror)
        .path(stroke = Ink, strokeLineWidth = 1.8f, strokeLineCap = StrokeCap.Round, strokeLineJoin = StrokeJoin.Round, pathBuilder = draw)
        .build()

private fun solid(name: String, size: Int = 24, draw: PathBuilder.() -> Unit): ImageVector =
    ImageVector.Builder(name, size.dp, size.dp, 24f, 24f)
        .path(fill = Ink, pathBuilder = draw)
        .build()

private fun PathBuilder.dot(x: Float, y: Float, radius: Float) {
    val control = radius * 0.55228475f
    moveTo(x + radius, y)
    curveTo(x + radius, y + control, x + control, y + radius, x, y + radius)
    curveTo(x - control, y + radius, x - radius, y + control, x - radius, y)
    curveTo(x - radius, y - control, x - control, y - radius, x, y - radius)
    curveTo(x + control, y - radius, x + radius, y - control, x + radius, y)
    close()
}

private fun PathBuilder.roundedBox(left: Float, top: Float, right: Float, bottom: Float, radius: Float) {
    moveTo(left + radius, top)
    lineTo(right - radius, top)
    quadTo(right, top, right, top + radius)
    lineTo(right, bottom - radius)
    quadTo(right, bottom, right - radius, bottom)
    lineTo(left + radius, bottom)
    quadTo(left, bottom, left, bottom - radius)
    lineTo(left, top + radius)
    quadTo(left, top, left + radius, top)
    close()
}

// Four unequal dots cluster around a seed. Keep the launcher foreground in sync.
private val ClusterMark = solid("Mote", size = 27) {
    dot(8.7f, 12.8f, 4.3f)
    dot(15.8f, 6.5f, 2.85f)
    dot(17.75f, 14.9f, 2.15f)
    dot(11f, 20f, 1.45f)
}

private val MoteGlyphs = mapOf(
    MoteGlyph.Settings to outline(MoteGlyph.Settings) {
        moveTo(9f, 4f)
        lineTo(9f, 8f)
        moveTo(9f, 12f)
        lineTo(9f, 20f)
        moveTo(15f, 4f)
        lineTo(15f, 12f)
        moveTo(15f, 16f)
        lineTo(15f, 20f)
        roundedBox(6.5f, 8f, 11.5f, 12f, 1f)
        roundedBox(12.5f, 12f, 17.5f, 16f, 1f)
    },
    MoteGlyph.Back to outline(MoteGlyph.Back, autoMirror = true) {
        moveTo(19.5f, 12f)
        lineTo(4.5f, 12f)
        moveTo(11f, 5.5f)
        lineTo(4.5f, 12f)
        lineTo(11f, 18.5f)
    },
    MoteGlyph.More to solid("More") {
        dot(12f, 5.5f, 1.45f)
        dot(12f, 12f, 1.45f)
        dot(12f, 18.5f, 1.45f)
    },
    MoteGlyph.Pin to outline(MoteGlyph.Pin) {
        moveTo(7f, 4f)
        lineTo(17f, 4f)
        moveTo(9f, 4f)
        lineTo(9f, 10.2f)
        lineTo(6.1f, 13.1f)
        lineTo(6.1f, 15f)
        lineTo(17.9f, 15f)
        lineTo(17.9f, 13.1f)
        lineTo(15f, 10.2f)
        lineTo(15f, 4f)
        moveTo(12f, 15f)
        lineTo(12f, 21f)
    },
    MoteGlyph.Archive to outline(MoteGlyph.Archive) {
        roundedBox(3f, 4.25f, 21f, 8.75f, 1.25f)
        moveTo(5f, 8.75f)
        lineTo(5f, 18.4f)
        quadTo(5f, 20f, 6.6f, 20f)
        lineTo(17.4f, 20f)
        quadTo(19f, 20f, 19f, 18.4f)
        lineTo(19f, 8.75f)
        moveTo(9.5f, 12.75f)
        lineTo(14.5f, 12.75f)
    },
    MoteGlyph.Restore to outline(MoteGlyph.Restore, autoMirror = true) {
        moveTo(4f, 4.5f)
        lineTo(4f, 10f)
        lineTo(9.5f, 10f)
        moveTo(4.1f, 9.8f)
        curveTo(5.7f, 6.6f, 8.4f, 4.5f, 12.2f, 4.5f)
        curveTo(16.7f, 4.5f, 20.5f, 8f, 20.5f, 12.4f)
        curveTo(20.5f, 16.8f, 16.9f, 20.2f, 12.5f, 20.2f)
        curveTo(9.2f, 20.2f, 6.5f, 18.5f, 5.1f, 16f)
    },
    MoteGlyph.Copy to outline(MoteGlyph.Copy) {
        roundedBox(9f, 9f, 20.5f, 20.5f, 2f)
        moveTo(6.2f, 15f)
        lineTo(5.5f, 15f)
        quadTo(3.5f, 15f, 3.5f, 13f)
        lineTo(3.5f, 5.5f)
        quadTo(3.5f, 3.5f, 5.5f, 3.5f)
        lineTo(13f, 3.5f)
        quadTo(15f, 3.5f, 15f, 5.5f)
        lineTo(15f, 6.2f)
    },
    MoteGlyph.Share to outline(MoteGlyph.Share) {
        dot(6f, 12f, 2.35f)
        dot(18f, 5.5f, 2.35f)
        dot(18f, 18.5f, 2.35f)
        moveTo(8.1f, 10.9f)
        lineTo(15.9f, 6.6f)
        moveTo(8.1f, 13.1f)
        lineTo(15.9f, 17.4f)
    },
    MoteGlyph.Delete to outline(MoteGlyph.Delete) {
        moveTo(4f, 6f)
        lineTo(20f, 6f)
        moveTo(9f, 6f)
        lineTo(9f, 3.6f)
        lineTo(15f, 3.6f)
        lineTo(15f, 6f)
        moveTo(6f, 6f)
        lineTo(6.8f, 18.5f)
        quadTo(7f, 20.5f, 8.8f, 20.5f)
        lineTo(15.2f, 20.5f)
        quadTo(17f, 20.5f, 17.2f, 18.5f)
        lineTo(18f, 6f)
        moveTo(10f, 10.5f)
        lineTo(10.3f, 16.5f)
        moveTo(14f, 10.5f)
        lineTo(13.7f, 16.5f)
    },
    MoteGlyph.Expand to outline(MoteGlyph.Expand) {
        moveTo(14f, 4.5f)
        lineTo(19.5f, 4.5f)
        lineTo(19.5f, 10f)
        moveTo(19.5f, 4.5f)
        lineTo(13.5f, 10.5f)
        moveTo(10f, 19.5f)
        lineTo(4.5f, 19.5f)
        lineTo(4.5f, 14f)
        moveTo(4.5f, 19.5f)
        lineTo(10.5f, 13.5f)
    },
    MoteGlyph.Send to outline(MoteGlyph.Send) {
        moveTo(12f, 20f)
        lineTo(12f, 4f)
        moveTo(5.5f, 10.5f)
        lineTo(12f, 4f)
        lineTo(18.5f, 10.5f)
    },
    MoteGlyph.Check to outline(MoteGlyph.Check) {
        moveTo(4.75f, 12f)
        lineTo(9.5f, 16.75f)
        lineTo(19.25f, 7f)
    },
    MoteGlyph.Close to outline(MoteGlyph.Close) {
        moveTo(6.25f, 6.25f)
        lineTo(17.75f, 17.75f)
        moveTo(6.25f, 17.75f)
        lineTo(17.75f, 6.25f)
    },
    MoteGlyph.Inbox to outline(MoteGlyph.Inbox) {
        moveTo(3.5f, 12f)
        lineTo(6.3f, 5.25f)
        lineTo(17.7f, 5.25f)
        lineTo(20.5f, 12f)
        lineTo(20.5f, 18f)
        quadTo(20.5f, 19.5f, 19f, 19.5f)
        lineTo(5f, 19.5f)
        quadTo(3.5f, 19.5f, 3.5f, 18f)
        close()
        moveTo(3.5f, 12f)
        lineTo(8.5f, 12f)
        lineTo(10.2f, 14.5f)
        lineTo(13.8f, 14.5f)
        lineTo(15.5f, 12f)
        lineTo(20.5f, 12f)
    },
    MoteGlyph.Edit to outline(MoteGlyph.Edit) {
        moveTo(4.5f, 19.5f)
        lineTo(5.4f, 14.7f)
        lineTo(15.7f, 4.4f)
        quadTo(16.9f, 3.2f, 18.1f, 4.4f)
        lineTo(19.6f, 5.9f)
        quadTo(20.8f, 7.1f, 19.6f, 8.3f)
        lineTo(9.3f, 18.6f)
        close()
        moveTo(14.1f, 6f)
        lineTo(18f, 9.9f)
    },
    MoteGlyph.Link to outline(MoteGlyph.Link) {
        moveTo(9.8f, 8.6f)
        lineTo(13.1f, 5.3f)
        curveTo(14.7f, 3.7f, 17.2f, 3.7f, 18.8f, 5.3f)
        curveTo(20.4f, 6.9f, 20.4f, 9.4f, 18.8f, 11f)
        lineTo(15.5f, 14.3f)
        moveTo(14.2f, 15.4f)
        lineTo(10.9f, 18.7f)
        curveTo(9.3f, 20.3f, 6.8f, 20.3f, 5.2f, 18.7f)
        curveTo(3.6f, 17.1f, 3.6f, 14.6f, 5.2f, 13f)
        lineTo(8.5f, 9.7f)
        moveTo(8.8f, 15.2f)
        lineTo(15.2f, 8.8f)
    },
    MoteGlyph.Alert to outline(MoteGlyph.Alert) {
        moveTo(10.6f, 4.9f)
        quadTo(12f, 2.5f, 13.4f, 4.9f)
        lineTo(20.6f, 17.5f)
        quadTo(21.7f, 19.5f, 19.4f, 19.5f)
        lineTo(4.6f, 19.5f)
        quadTo(2.3f, 19.5f, 3.4f, 17.5f)
        close()
        moveTo(12f, 9f)
        lineTo(12f, 12.7f)
        moveTo(12f, 15.9f)
        lineTo(12f, 16f)
    },
    MoteGlyph.Dots to solid("Dots") {
        dot(5.5f, 12f, 1.45f)
        dot(12f, 12f, 1.45f)
        dot(18.5f, 12f, 1.45f)
    },
    MoteGlyph.Sync to outline(MoteGlyph.Sync) {
        moveTo(20f, 12f)
        curveTo(20f, 7.6f, 16.4f, 4f, 12f, 4f)
        curveTo(8.4f, 4f, 5.2f, 6.2f, 4f, 9.2f)
        moveTo(4f, 4.5f)
        lineTo(4f, 9.5f)
        lineTo(9f, 9.5f)
        moveTo(4f, 12f)
        curveTo(4f, 16.4f, 7.6f, 20f, 12f, 20f)
        curveTo(15.6f, 20f, 18.8f, 17.8f, 20f, 14.8f)
        moveTo(20f, 19.5f)
        lineTo(20f, 14.5f)
        lineTo(15f, 14.5f)
    },
    MoteGlyph.Reply to outline(MoteGlyph.Reply) {
        moveTo(9.5f, 7f)
        lineTo(4.5f, 12f)
        lineTo(9.5f, 17f)
        moveTo(5f, 12f)
        lineTo(13.5f, 12f)
        curveTo(17.2f, 12f, 19.5f, 14.5f, 19.5f, 18f)
    },
    MoteGlyph.Fold to outline(MoteGlyph.Fold) {
        moveTo(6f, 14.5f)
        lineTo(12f, 8.5f)
        lineTo(18f, 14.5f)
    },
    MoteGlyph.Unfold to outline(MoteGlyph.Unfold) {
        moveTo(6f, 9.5f)
        lineTo(12f, 15.5f)
        lineTo(18f, 9.5f)
    },
    MoteGlyph.Info to outline(MoteGlyph.Info) {
        dot(12f, 12f, 8f)
        moveTo(12f, 10.5f)
        lineTo(12f, 16.5f)
        dot(12f, 7.5f, 0.8f)
    },
)
