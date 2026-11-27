package org.papercraft

import android.content.Context
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

enum class ThemeMode(val label: String) { System("System"), Light("Light"), Dark("Dark"), Black("Black") }

/** App settings, one SharedPreferences file. */
class Settings(context: Context) {
    private val prefs = context.getSharedPreferences("settings", Context.MODE_PRIVATE)

    var theme: ThemeMode
        get() = prefs.getString("theme", null)?.let { name -> ThemeMode.entries.firstOrNull { it.name == name } } ?: ThemeMode.System
        set(value) = prefs.edit().putString("theme", value.name).apply()

    var recentsAsGrid: Boolean
        get() = prefs.getBoolean("recentsAsGrid", true)
        set(value) = prefs.edit().putBoolean("recentsAsGrid", value).apply()
}

// Seed #2B579A, from Material Theme Builder.
private val LightColors = lightColorScheme(
    primary = Color(0xFF2B579A), onPrimary = Color.White,
    primaryContainer = Color(0xFFD6E3FF), onPrimaryContainer = Color(0xFF001B3E),
    secondary = Color(0xFF565F71), secondaryContainer = Color(0xFFDAE2F9), onSecondaryContainer = Color(0xFF131C2B),
    background = Color(0xFFF9F9FF), surface = Color(0xFFF9F9FF), onSurface = Color(0xFF191C20),
    surfaceVariant = Color(0xFFE0E2EC), onSurfaceVariant = Color(0xFF44474E),
    surfaceContainerLowest = Color.White, surfaceContainerLow = Color(0xFFF3F3FA), surfaceContainer = Color(0xFFEDEDF4),
    surfaceContainerHigh = Color(0xFFE7E8EE), surfaceContainerHighest = Color(0xFFE2E2E9),
    outline = Color(0xFF74777F), outlineVariant = Color(0xFFC4C6D0),
    inverseSurface = Color(0xFF2E3036), inverseOnSurface = Color(0xFFF0F0F7), inversePrimary = Color(0xFFAAC7FF),
)
private val DarkColors = darkColorScheme(
    primary = Color(0xFFAAC7FF), onPrimary = Color(0xFF0A305F),
    primaryContainer = Color(0xFF284777), onPrimaryContainer = Color(0xFFD6E3FF),
    secondary = Color(0xFFBEC6DC), secondaryContainer = Color(0xFF3E4759), onSecondaryContainer = Color(0xFFDAE2F9),
    background = Color(0xFF111318), surface = Color(0xFF111318), onSurface = Color(0xFFE2E2E9),
    surfaceVariant = Color(0xFF44474E), onSurfaceVariant = Color(0xFFC4C6D0),
    surfaceContainerLowest = Color(0xFF0C0E13), surfaceContainerLow = Color(0xFF191C20), surfaceContainer = Color(0xFF1D2024),
    surfaceContainerHigh = Color(0xFF282A2F), surfaceContainerHighest = Color(0xFF33353A),
    outline = Color(0xFF8E9099), outlineVariant = Color(0xFF44474E),
    inverseSurface = Color(0xFFE2E2E9), inverseOnSurface = Color(0xFF2E3036), inversePrimary = Color(0xFF2B579A),
)
// True black where the screen is mostly background, so OLED pixels switch off; raised surfaces stay just above it.
private val BlackColors = DarkColors.copy(
    background = Color.Black, surface = Color.Black,
    surfaceContainerLowest = Color.Black, surfaceContainerLow = Color(0xFF0B0C0F), surfaceContainer = Color(0xFF111216),
    surfaceContainerHigh = Color(0xFF1A1B20), surfaceContainerHighest = Color(0xFF24252A),
)

/** One radius scale: controls 12, cards 20, sheets 28. */
private val PapercraftShapes = Shapes(
    extraSmall = RoundedCornerShape(8.dp),
    small = RoundedCornerShape(12.dp),
    medium = RoundedCornerShape(20.dp),
    large = RoundedCornerShape(28.dp),
    extraLarge = RoundedCornerShape(28.dp),
)

private val PapercraftType = Typography().run {
    copy(
        titleLarge = titleLarge.copy(fontWeight = FontWeight.SemiBold),
        titleMedium = titleMedium.copy(fontWeight = FontWeight.SemiBold),
        titleSmall = titleSmall.copy(fontWeight = FontWeight.SemiBold),
    )
}

fun ThemeMode.isDark(systemDark: Boolean) = when (this) {
    ThemeMode.System -> systemDark
    ThemeMode.Light -> false
    ThemeMode.Dark, ThemeMode.Black -> true
}

@Composable
fun PapercraftTheme(mode: ThemeMode, content: @Composable () -> Unit) {
    val colors = when {
        mode == ThemeMode.Black -> BlackColors
        mode.isDark(isSystemInDarkTheme()) -> DarkColors
        else -> LightColors
    }
    MaterialTheme(colorScheme = colors, shapes = PapercraftShapes, typography = PapercraftType, content = content)
}
