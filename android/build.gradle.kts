plugins {
    id("com.android.application") version "9.5.3" apply false
    // AGP 9 has Kotlin built in; this line only pins the Kotlin version.
    id("org.jetbrains.kotlin.android") version "2.4.21" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.21" apply false
}
