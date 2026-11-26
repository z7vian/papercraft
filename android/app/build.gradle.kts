plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "org.papercraft"
    compileSdk { version = release(37) { minorApiLevel = 2 } }
    ndkVersion = "30.0.16248370"

    defaultConfig {
        applicationId = "org.papercraft"
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0"
    }
    buildFeatures { compose = true }
}

/** Runs `cargo <cargoArgs>` in core/. Cargo tracks its own changes, so this always runs. */
abstract class Cargo : DefaultTask() {
    @get:Input abstract val cargoArgs: ListProperty<String>
    @get:Internal abstract val coreDir: DirectoryProperty
    @get:Internal abstract val ndkDir: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty
    @get:Inject abstract val exec: ExecOperations
    init { outputs.upToDateWhen { false } }

    @TaskAction fun run() {
        exec.exec {
            workingDir = coreDir.get().asFile
            environment("ANDROID_NDK_HOME", ndkDir.get().asFile)
            commandLine(listOf("cargo") + cargoArgs.get())
        }
    }
}

val rustLibs = layout.buildDirectory.dir("rust/jniLibs")
val rustBindings = layout.buildDirectory.dir("rust/kotlin")
tasks.withType<Cargo>().configureEach {
    coreDir.set(rootProject.layout.projectDirectory.dir("../core"))
    ndkDir.set(androidComponents.sdkComponents.ndkDirectory)
}

val buildRust = tasks.register<Cargo>("buildRust") {
    outputDir.set(rustLibs)
    cargoArgs.set(listOf(
        "ndk", "-t", "arm64-v8a", "-t", "x86_64", "--platform", "26",
        "-o", rustLibs.get().asFile.path, "build", "--release",
    ))
}

val rustKotlinBindings = tasks.register<Cargo>("rustKotlinBindings") {
    dependsOn(buildRust)
    outputDir.set(rustBindings)
    cargoArgs.set(listOf(
        "run", "-q", "--features", "cli", "--bin", "uniffi-bindgen", "--", "generate",
        "--library", "target/aarch64-linux-android/release/libpapercraft_core.so",
        "--language", "kotlin", "--no-format", "--out-dir", rustBindings.get().asFile.path,
    ))
}

androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addGeneratedSourceDirectory(buildRust, Cargo::outputDir)
        variant.sources.kotlin?.addGeneratedSourceDirectory(rustKotlinBindings, Cargo::outputDir)
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.11.01"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("net.java.dev.jna:jna:5.19.1@aar") // UniFFI's Kotlin code calls Rust through JNA
}
