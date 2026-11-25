plugins {
    alias(libs.plugins.android.library)
}

android {
    namespace = "org.harmion.sdk"
    compileSdk = 37
    compileSdkMinor = 2
    buildToolsVersion = "37.0.0"

    defaultConfig {
        minSdk = 29
        consumerProguardFiles("consumer-rules.pro")
        // SHA-256 of the node app's signing certificate. Release builds must
        // pass -PharmionNodeCertSha256=<release cert digest>.
        val nodeCert =
            (project.findProperty("harmionNodeCertSha256") as String?)
                ?: "4870b91c1614baa3baf71ebdb0070bf5bc7711d74d60ee1ea15ab439ca8b0992"
        buildConfigField("String", "NODE_CERT_SHA256", "\"$nodeCert\"")
    }

    buildFeatures {
        aidl = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    publishing {
        singleVariant("release")
    }
}


dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.okhttp)
    testImplementation(kotlin("test"))
}
