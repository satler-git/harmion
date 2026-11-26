import java.time.OffsetDateTime
import java.time.format.DateTimeFormatter
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

val versionProps = Properties().apply {
    project.file("version.properties").inputStream().use { load(it) }
}

// GitHub Actions release workflow sets this. Local builds (including a local
// signed release) are development artifacts and carry -dev.
val officialRelease = System.getenv("HARMION_OFFICIAL_RELEASE") == "true"

fun git(vararg args: String): String =
    runCatching {
        providers.exec {
            commandLine("git", *args)
            workingDir = rootProject.projectDir.parentFile
        }.standardOutput.asText.get().trim()
    }.getOrDefault("")

val gitRevision = git("rev-parse", "--short=12", "HEAD").ifEmpty { "unknown" }
val gitBranch = git("rev-parse", "--abbrev-ref", "HEAD").ifEmpty { "unknown" }
val gitDirty = git("status", "--porcelain").isNotEmpty()
val buildTime = OffsetDateTime.now().format(DateTimeFormatter.ISO_OFFSET_DATE_TIME)

android {
    namespace = "org.harmion.android"
    compileSdk = 37
    compileSdkMinor = 2
    buildToolsVersion = "37.0.0"

    defaultConfig {
        applicationId = "dev.satler.harmion"
        minSdk = 29
        targetSdk = 36
        versionCode = versionProps.getProperty("versionCode").toInt()
        versionName = versionProps.getProperty("versionName")
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
        buildConfigField("String", "GIT_REVISION", "\"$gitRevision\"")
        buildConfigField("String", "GIT_BRANCH", "\"$gitBranch\"")
        buildConfigField("boolean", "GIT_DIRTY", gitDirty.toString())
        buildConfigField("String", "BUILD_TIME", "\"$buildTime\"")
        buildConfigField("boolean", "OFFICIAL_RELEASE", officialRelease.toString())
    }

    signingConfigs {
        create("release") {
            val path = System.getenv("HARMION_KEYSTORE")
            if (!path.isNullOrBlank()) {
                storeFile = file(path)
                storePassword = System.getenv("HARMION_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("HARMION_KEY_ALIAS") ?: "harmion"
                keyPassword = System.getenv("HARMION_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        debug {
            versionNameSuffix = "-dev"
        }
        release {
            if (!officialRelease) {
                versionNameSuffix = "-dev"
            }
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            signingConfig =
                if (!System.getenv("HARMION_KEYSTORE").isNullOrBlank()) {
                    signingConfigs.getByName("release")
                } else {
                    null
                }
        }
    }

    buildFeatures {
        compose = true
        aidl = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
    }
}


dependencies {
    implementation(project(":sdk"))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.appcompat)
    implementation(libs.androidx.biometric)
    implementation(libs.androidx.fragment.ktx)
    implementation(libs.androidx.work.runtime)
    implementation(libs.kotlinx.coroutines.android)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons)
    implementation(libs.zxing.embedded)
    implementation(libs.zxing.core)
    implementation(libs.androidx.glance.appwidget)
    debugImplementation(libs.androidx.compose.ui.tooling)
}
