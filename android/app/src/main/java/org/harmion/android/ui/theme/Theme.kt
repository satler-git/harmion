package org.harmion.android.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// Brand palette used when dynamic color is unavailable or disabled. Dynamic
// color (Material You) is preferred on Android 12+; these values are the
// Harmion identity, a deep indigo with a teal accent.
private val LightColors =
    lightColorScheme(
        primary = Color(0xFF4355C8),
        onPrimary = Color(0xFFFFFFFF),
        primaryContainer = Color(0xFFDFE0FF),
        onPrimaryContainer = Color(0xFF00105C),
        secondary = Color(0xFF5B5D72),
        onSecondary = Color(0xFFFFFFFF),
        secondaryContainer = Color(0xFFE0E1F9),
        onSecondaryContainer = Color(0xFF181A2C),
        tertiary = Color(0xFF00696E),
        onTertiary = Color(0xFFFFFFFF),
        tertiaryContainer = Color(0xFF9CF1F7),
        onTertiaryContainer = Color(0xFF002022),
        background = Color(0xFFFBF8FF),
        onBackground = Color(0xFF1B1B21),
        surface = Color(0xFFFBF8FF),
        onSurface = Color(0xFF1B1B21),
        surfaceVariant = Color(0xFFE3E1EC),
        onSurfaceVariant = Color(0xFF46464F),
        surfaceContainerLowest = Color(0xFFFFFFFF),
        surfaceContainerLow = Color(0xFFF5F2FA),
        surfaceContainer = Color(0xFFEFEDF4),
        surfaceContainerHigh = Color(0xFFE9E7EF),
        surfaceContainerHighest = Color(0xFFE3E1E9),
        outline = Color(0xFF767680),
        outlineVariant = Color(0xFFC6C5D0),
    )

private val DarkColors =
    darkColorScheme(
        primary = Color(0xFFBCC2FF),
        onPrimary = Color(0xFF0B1B8F),
        primaryContainer = Color(0xFF2938A9),
        onPrimaryContainer = Color(0xFFDFE0FF),
        secondary = Color(0xFFC4C5DD),
        onSecondary = Color(0xFF2D2F42),
        secondaryContainer = Color(0xFF434659),
        onSecondaryContainer = Color(0xFFE0E1F9),
        tertiary = Color(0xFF80D4DA),
        onTertiary = Color(0xFF00373A),
        tertiaryContainer = Color(0xFF004F53),
        onTertiaryContainer = Color(0xFF9CF1F7),
        background = Color(0xFF121318),
        onBackground = Color(0xFFE4E1E9),
        surface = Color(0xFF121318),
        onSurface = Color(0xFFE4E1E9),
        surfaceVariant = Color(0xFF46464F),
        onSurfaceVariant = Color(0xFFC6C5D0),
        surfaceContainerLowest = Color(0xFF0D0E13),
        surfaceContainerLow = Color(0xFF1A1B21),
        surfaceContainer = Color(0xFF1E1F25),
        surfaceContainerHigh = Color(0xFF292A30),
        surfaceContainerHighest = Color(0xFF34343B),
        outline = Color(0xFF90909A),
        outlineVariant = Color(0xFF46464F),
    )

private val HarmionTypography =
    Typography(
        displaySmall =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.Bold,
                fontSize = 38.sp,
                lineHeight = 46.sp,
                letterSpacing = (-0.6).sp,
            ),
        headlineLarge =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.Bold,
                fontSize = 32.sp,
                lineHeight = 40.sp,
                letterSpacing = (-0.4).sp,
            ),
        headlineMedium =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.SemiBold,
                fontSize = 26.sp,
                lineHeight = 34.sp,
            ),
        headlineSmall =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.SemiBold,
                fontSize = 22.sp,
                lineHeight = 30.sp,
            ),
        titleLarge =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.SemiBold,
                fontSize = 21.sp,
                lineHeight = 28.sp,
            ),
        titleMedium =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.SemiBold,
                fontSize = 16.sp,
                lineHeight = 24.sp,
            ),
        titleSmall =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.Medium,
                fontSize = 14.sp,
                lineHeight = 20.sp,
            ),
        bodyLarge =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.Normal,
                fontSize = 16.sp,
                lineHeight = 26.sp,
            ),
        bodyMedium =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.Normal,
                fontSize = 14.sp,
                lineHeight = 22.sp,
            ),
        bodySmall =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.Normal,
                fontSize = 12.sp,
                lineHeight = 18.sp,
            ),
        labelLarge =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.SemiBold,
                fontSize = 14.sp,
                lineHeight = 20.sp,
            ),
        labelMedium =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.Medium,
                fontSize = 12.sp,
                lineHeight = 16.sp,
            ),
        labelSmall =
            TextStyle(
                fontFamily = FontFamily.Default,
                fontWeight = FontWeight.Medium,
                fontSize = 11.sp,
                lineHeight = 16.sp,
            ),
    )

private val HarmionShapes =
    Shapes(
        extraSmall = androidx.compose.foundation.shape.RoundedCornerShape(8.dp),
        small = androidx.compose.foundation.shape.RoundedCornerShape(12.dp),
        medium = androidx.compose.foundation.shape.RoundedCornerShape(18.dp),
        large = androidx.compose.foundation.shape.RoundedCornerShape(26.dp),
        extraLarge = androidx.compose.foundation.shape.RoundedCornerShape(32.dp),
    )

/**
 * Material 3 has no semantic "success" role. These tones are deliberately green
 * in both themes, because a completed sync must be recognizable at a glance
 * even when the user picked a non-green dynamic palette.
 */
object HarmionSuccess {
    val container: Color
        @Composable get() = if (isSystemInDarkTheme()) Color(0xFF14532D) else Color(0xFF166534)

    val onContainer: Color
        @Composable get() = if (isSystemInDarkTheme()) Color(0xFFBBF7D0) else Color(0xFFFFFFFF)

    val accent: Color
        @Composable get() = if (isSystemInDarkTheme()) Color(0xFF86EFAC) else Color(0xFF15803D)

    val accentContainer: Color
        @Composable get() = if (isSystemInDarkTheme()) Color(0xFF1F3D2E) else Color(0xFFDCFCE7)
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun HarmionTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    dynamicColor: Boolean = true,
    content: @Composable () -> Unit,
) {
    val context = LocalContext.current
    val colors =
        when {
            dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
                if (darkTheme) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
            darkTheme -> DarkColors
            else -> LightColors
        }
    MaterialTheme(
        colorScheme = colors,
        typography = HarmionTypography,
        shapes = HarmionShapes,
        motionScheme = MotionScheme.expressive(),
        content = content,
    )
}
