package org.harmion.android.ui

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.util.AttributeSet
import com.journeyapps.barcodescanner.ViewfinderView

/**
 * Viewfinder with an explicit white frame and corner brackets.
 *
 * The stock view only dims the area outside the scan window, which made it
 * unclear where a code should be placed. The frame is drawn from the camera's
 * real framing rectangle, so it always matches the decoded area.
 */
class HarmionViewfinderView(context: Context, attrs: AttributeSet?) :
    ViewfinderView(context, attrs) {

    private val density = resources.displayMetrics.density
    private val framePaint =
        Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeWidth = 1.5f * density
            color = Color.argb(190, 255, 255, 255)
        }
    private val cornerPaint =
        Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeWidth = 5f * density
            strokeCap = Paint.Cap.ROUND
            strokeJoin = Paint.Join.ROUND
            color = Color.WHITE
        }

    init {
        setMaskColor(Color.argb(168, 0, 0, 0))
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val rect = framingRect ?: return
        val inset = density
        val frame =
            RectF(
                rect.left + inset,
                rect.top + inset,
                rect.right - inset,
                rect.bottom - inset,
            )
        val radius = 20f * density
        canvas.drawRoundRect(frame, radius, radius, framePaint)

        val length = 30f * density
        drawCorner(canvas, frame.left, frame.top, 1f, 1f, length)
        drawCorner(canvas, frame.right, frame.top, -1f, 1f, length)
        drawCorner(canvas, frame.left, frame.bottom, 1f, -1f, length)
        drawCorner(canvas, frame.right, frame.bottom, -1f, -1f, length)
    }

    private fun drawCorner(
        canvas: Canvas,
        x: Float,
        y: Float,
        horizontal: Float,
        vertical: Float,
        length: Float,
    ) {
        val path =
            Path().apply {
                moveTo(x + horizontal * length, y)
                lineTo(x, y)
                lineTo(x, y + vertical * length)
            }
        canvas.drawPath(path, cornerPaint)
    }
}
