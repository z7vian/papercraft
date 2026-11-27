import java.net.URI
import java.security.MessageDigest

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
    // One debug key for every machine and CI run, so a new debug APK installs over the old one.
    signingConfigs { getByName("debug") { storeFile = file("debug.keystore") } }
    // Limit: release builds use the debug key, for speed tests only; a real upload key lifts it before any store release.
    buildTypes { release { signingConfig = signingConfigs.getByName("debug") } }
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
    // cargo-ndk copies every .so the build makes (some engine crates are cdylibs too); the app loads only ours.
    doLast {
        rustLibs.get().asFile.walk().filter { it.isFile && it.name != "libpapercraft_core.so" }.forEach { it.delete() }
    }
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

/**
 * Downloads one bblanchon/pdfium-binaries archive per ABI, checks its SHA-256 and unpacks its libpdfium.so
 * into [outputDir]/<abi>/. Gradle skips it while the inputs and the output are unchanged.
 */
abstract class PdfiumLibs : DefaultTask() {
    /** The chromium/<release> tag; it must match the `pdfium_<release>` feature of pdfium-render in core/Cargo.toml. */
    @get:Input abstract val release: Property<String>
    /** ABI to "<archive name> <sha256>". */
    @get:Input abstract val archives: MapProperty<String, String>
    @get:OutputDirectory abstract val outputDir: DirectoryProperty
    @get:Inject abstract val archiveOps: ArchiveOperations
    @get:Inject abstract val fileOps: FileSystemOperations

    @TaskAction fun run() {
        val out = outputDir.get().asFile
        fileOps.delete { delete(out) }
        for ((abi, spec) in archives.get()) {
            val (name, sha256) = spec.split(' ')
            val url = "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/${release.get()}/$name"
            val tgz = File(temporaryDir, name)
            URI(url).toURL().openStream().use { input -> tgz.outputStream().use { input.copyTo(it) } }
            val actual = MessageDigest.getInstance("SHA-256").digest(tgz.readBytes()).joinToString("") { "%02x".format(it) }
            check(actual == sha256) { "$url has SHA-256 $actual, expected $sha256" }
            fileOps.copy {
                from(archiveOps.tarTree(archiveOps.gzip(tgz)))
                include("lib/libpdfium.so")
                eachFile { path = "libpdfium.so" }
                includeEmptyDirs = false
                into(File(out, abi))
            }
        }
    }
}

val pdfiumLibs = tasks.register<PdfiumLibs>("pdfiumLibs") {
    release.set("8125")
    archives.set(mapOf(
        "arm64-v8a" to "pdfium-android-arm64.tgz 0495e57163429937caaee1bdd9b07924a7098dbd51d1b97645d124e349f097b4",
        "x86_64" to "pdfium-android-x64.tgz bd68707f29f2316c20f9d5c793b8731796a4a65408ac4d3c30bc606390354e41",
    ))
    outputDir.set(layout.buildDirectory.dir("pdfium/jniLibs"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addGeneratedSourceDirectory(buildRust, Cargo::outputDir)
        variant.sources.jniLibs?.addGeneratedSourceDirectory(pdfiumLibs, PdfiumLibs::outputDir)
        variant.sources.kotlin?.addGeneratedSourceDirectory(rustKotlinBindings, Cargo::outputDir)
    }
}

// Lists every JVM unit test in the CI log.
tasks.withType<Test>().configureEach {
    testLogging {
        events("passed", "skipped", "failed")
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.11.01"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("net.java.dev.jna:jna:5.19.1@aar") // UniFFI's Kotlin code calls Rust through JNA
    testImplementation("junit:junit:4.13.2")
}
