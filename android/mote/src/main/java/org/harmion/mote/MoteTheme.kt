package org.harmion.mote

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// Keep the window background in mote_theme.xml in sync with this base surface.
private val Ink = Color(0xFF14111D)
private val MistWhite = Color(0xFFFAF6FF)
private val Lavender = Color(0xFFD9C0FF)
private val Lilac = Color(0xFFD8CAEB)
private val Mauve = Color(0xFFE6BDD4)

private val MoteColors =
    darkColorScheme(
        primary = Lavender,
        onPrimary = Color(0xFF322148),
        primaryContainer = Color(0xFF433455),
        onPrimaryContainer = Color(0xFFF0E3FF),
        inversePrimary = Color(0xFF654995),
        secondary = Lilac,
        onSecondary = Color(0xFF342943),
        secondaryContainer = Color(0xFF4C3D62),
        onSecondaryContainer = Color(0xFFEEDFFF),
        tertiary = Mauve,
        onTertiary = Color(0xFF45283B),
        tertiaryContainer = Color(0xFF502F44),
        onTertiaryContainer = Color(0xFFF8D9EB),
        background = Ink,
        onBackground = MistWhite,
        surface = Ink,
        onSurface = MistWhite,
        surfaceVariant = Color(0xFF342C43),
        onSurfaceVariant = Color(0xFFD0C4E0),
        surfaceTint = Color(0xFFB6A1D1),
        surfaceDim = Ink,
        surfaceBright = Color(0xFF49405A),
        surfaceContainerLowest = Color(0xFF100D18),
        surfaceContainerLow = Color(0xFF191624),
        surfaceContainer = Color(0xFF201B2B),
        surfaceContainerHigh = Color(0xFF2A2438),
        surfaceContainerHighest = Color(0xFF383047),
        inverseSurface = MistWhite,
        inverseOnSurface = Color(0xFF30283D),
        outline = Color(0xFF9989AD),
        outlineVariant = Color(0xFF483D5A),
        error = Color(0xFFFFB8C8),
        onError = Color(0xFF591B2D),
        errorContainer = Color(0xFF743346),
        onErrorContainer = Color(0xFFFFD9E2),
        scrim = Color.Black,
        primaryFixed = Color(0xFFF0E3FF),
        primaryFixedDim = Lavender,
        onPrimaryFixed = Color(0xFF231439),
        onPrimaryFixedVariant = Color(0xFF433455),
        secondaryFixed = Color(0xFFEEDFFF),
        secondaryFixedDim = Lilac,
        onSecondaryFixed = Color(0xFF20172C),
        onSecondaryFixedVariant = Color(0xFF4C3D62),
        tertiaryFixed = Color(0xFFF8D9EB),
        tertiaryFixedDim = Mauve,
        onTertiaryFixed = Color(0xFF341A2B),
        onTertiaryFixedVariant = Color(0xFF502F44),
    )

private fun moteText(size: Int, height: Int, weight: FontWeight, tracking: Float = 0f) =
    TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = weight,
        fontSize = size.sp,
        lineHeight = height.sp,
        letterSpacing = tracking.sp,
    )

private val MoteTypography =
    Typography(
        displayLarge = moteText(52, 60, FontWeight.SemiBold, -1f),
        displayMedium = moteText(43, 51, FontWeight.SemiBold, -0.8f),
        displaySmall = moteText(35, 43, FontWeight.SemiBold, -0.6f),
        headlineLarge = moteText(31, 40, FontWeight.SemiBold, -0.5f),
        headlineMedium = moteText(27, 35, FontWeight.SemiBold, -0.35f),
        headlineSmall = moteText(24, 32, FontWeight.SemiBold, -0.25f),
        titleLarge = moteText(21, 28, FontWeight.SemiBold, -0.25f),
        titleMedium = moteText(17, 24, FontWeight.SemiBold),
        titleSmall = moteText(15, 22, FontWeight.SemiBold, 0.05f),
        bodyLarge = moteText(17, 26, FontWeight.Normal, 0.1f),
        bodyMedium = moteText(15, 23, FontWeight.Normal, 0.15f),
        bodySmall = moteText(13, 19, FontWeight.Normal, 0.2f),
        labelLarge = moteText(14, 20, FontWeight.SemiBold, 0.1f),
        labelMedium = moteText(12, 17, FontWeight.Medium, 0.2f),
        labelSmall = moteText(11, 16, FontWeight.Medium, 0.3f),
    )

private val MoteShapes =
    Shapes(
        extraSmall = RoundedCornerShape(6.dp),
        small = RoundedCornerShape(10.dp),
        medium = RoundedCornerShape(14.dp),
        large = RoundedCornerShape(20.dp),
        extraLarge = RoundedCornerShape(28.dp),
    )

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun MoteTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = MoteColors,
        typography = MoteTypography,
        shapes = MoteShapes,
        motionScheme = MotionScheme.expressive(),
        content = content,
    )
}
