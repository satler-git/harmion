import org.gradle.api.tasks.Exec

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.kotlin.compose) apply false
}

// The Rust JNI libraries are build outputs, not sources. Make every Android
// module depend on them so a clean checkout cannot produce an APK without the
// native side. Run this from `nix develop .#android`.
val buildRust = tasks.register<Exec>("buildRust") {
    group = "build"
    description = "Build the Harmion JNI libraries for the Android ABIs"
    workingDir = rootDir
    val abis = (project.findProperty("harmionRustAbis") as String?)
        ?.split(',')
        ?.filter { it.isNotBlank() }
        ?: listOf("arm64-v8a", "x86_64")
    commandLine(listOf("bash", "scripts/build-rust.sh") + abis)
    environment("ANDROID_NDK_ROOT", System.getenv("ANDROID_NDK_ROOT") ?: "")
}

subprojects {
    tasks.matching { it.name == "preBuild" }.configureEach {
        dependsOn(buildRust)
    }
}
