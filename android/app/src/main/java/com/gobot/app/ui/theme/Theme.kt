package com.gobot.app.ui.theme

import android.app.Activity
import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat

// GoBot brand colors
private val GoBotPrimary = Color(0xFF6366F1)  // Indigo
private val GoBotSecondary = Color(0xFF8B5CF6)  // Purple
private val GoBotTertiary = Color(0xFF06B6D4)  // Cyan

private val DarkColorScheme = darkColorScheme(
    primary = GoBotPrimary,
    secondary = GoBotSecondary,
    tertiary = GoBotTertiary,
    background = Color(0xFF0F0F23),
    surface = Color(0xFF1A1A2E),
    surfaceVariant = Color(0xFF252540),
    onPrimary = Color.White,
    onSecondary = Color.White,
    onTertiary = Color.Black,
    onBackground = Color(0xFFE4E4E7),
    onSurface = Color(0xFFE4E4E7),
    onSurfaceVariant = Color(0xFFA1A1AA)
)

private val LightColorScheme = lightColorScheme(
    primary = GoBotPrimary,
    secondary = GoBotSecondary,
    tertiary = GoBotTertiary,
    background = Color(0xFFFAFAFA),
    surface = Color.White,
    surfaceVariant = Color(0xFFF4F4F5),
    onPrimary = Color.White,
    onSecondary = Color.White,
    onTertiary = Color.White,
    onBackground = Color(0xFF18181B),
    onSurface = Color(0xFF18181B),
    onSurfaceVariant = Color(0xFF71717A)
)

@Composable
fun GoBotTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    dynamicColor: Boolean = false,  // Disabled to use brand colors
    content: @Composable () -> Unit
) {
    val colorScheme = when {
        dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            val context = LocalContext.current
            if (darkTheme) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        }
        darkTheme -> DarkColorScheme
        else -> LightColorScheme
    }

    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as Activity).window
            window.statusBarColor = colorScheme.background.toArgb()
            WindowCompat.getInsetsController(window, view).isAppearanceLightStatusBars = !darkTheme
        }
    }

    MaterialTheme(
        colorScheme = colorScheme,
        typography = Typography,
        content = content
    )
}
