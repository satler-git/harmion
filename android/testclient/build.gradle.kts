plugins {
    alias(libs.plugins.android.application)
}

android {
    namespace = "org.harmion.testclient"
    compileSdk = 37
    compileSdkMinor = 2
    buildToolsVersion = "37.0.0"

    defaultConfig {
        applicationId = "org.harmion.testclient"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.0.1"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation(project(":sdk"))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
}
