package com.akily.p2pshare.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable

private val AppDarkColors = darkColorScheme(
    primary = NeonCyan,
    onPrimary = NightBase,
    primaryContainer = NightSurfaceSoft,
    onPrimaryContainer = NightText,
    secondary = ElectricBlue,
    onSecondary = NightBase,
    secondaryContainer = NightLayer,
    onSecondaryContainer = NightText,
    tertiary = SignalGreen,
    onTertiary = NightBase,
    tertiaryContainer = NightLayer,
    onTertiaryContainer = NightText,
    background = NightBase,
    onBackground = NightText,
    surface = NightSurface,
    onSurface = NightText,
    surfaceVariant = NightSurfaceSoft,
    onSurfaceVariant = NightTextMuted,
    outline = NightBorder,
    outlineVariant = NightBorderStrong,
    error = SignalRed,
    onError = NightBase,
    errorContainer = SignalRed.copy(alpha = 0.16f),
    onErrorContainer = SignalRed,
)

@Composable
fun P2PShareTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = AppDarkColors,
        typography = AppTypography,
        content = content,
    )
}
