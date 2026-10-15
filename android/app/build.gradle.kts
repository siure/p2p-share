fun propOrEnv(name: String): String? {
    val fromProp = (project.findProperty(name) as String?)?.takeIf { it.isNotBlank() }
    val fromEnv = System.getenv(name)?.takeIf { it.isNotBlank() }
    return fromProp ?: fromEnv
}

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    val releaseKeystorePath = propOrEnv("ANDROID_KEYSTORE_PATH")
    val releaseKeystorePassword = propOrEnv("ANDROID_KEYSTORE_PASSWORD")
    val releaseKeyAlias = propOrEnv("ANDROID_KEY_ALIAS")
    val releaseKeyPassword = propOrEnv("ANDROID_KEY_PASSWORD")
    val hasReleaseSigning = listOf(
        releaseKeystorePath,
        releaseKeystorePassword,
        releaseKeyAlias,
        releaseKeyPassword
    ).all { !it.isNullOrBlank() }

    namespace = "com.akily.p2pshare"
    compileSdk = 35
    ndkVersion = "27.0.12077973"
    defaultConfig {
        applicationId = "com.akily.p2pshare"
        minSdk = 29
        targetSdk = 35
        versionName = "0.5.0"
        // Preserve semantic version ordering without a separate release counter.
        val parts = versionName!!.split('.').map(String::toInt)
        require(parts.size == 3 && parts.all { it in 0..999 })
        versionCode = parts[0] * 1_000_000 + parts[1] * 1_000 + parts[2]

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        vectorDrawables {
            useSupportLibrary = true
        }

        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    signingConfigs {
        if (hasReleaseSigning) {
            create("ciRelease") {
                storeFile = file(releaseKeystorePath!!)
                storePassword = releaseKeystorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            if (hasReleaseSigning) {
                signingConfig = signingConfigs.getByName("ciRelease")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    packaging {
        jniLibs {
            // These dependency artifacts are linked statically into our JNI library.
            excludes += listOf("**/libiroh-*.so", "**/libiroh_relay-*.so")
        }
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }

    sourceSets {
        getByName("main") {
            jniLibs.setSrcDirs(listOf(layout.buildDirectory.dir("generated/rustJniLibs")))
        }
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2025.01.00")

    implementation(composeBom)
    androidTestImplementation(composeBom)

    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.activity:activity-compose:1.10.0")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.documentfile:documentfile:1.0.1")
    implementation("com.google.zxing:core:3.5.3")
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    implementation("com.google.android.material:material:1.12.0")

    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")

    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.6.1")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
}

// Native code is a build input, rather than a manually refreshed checked-in binary.
val generatedJni = layout.buildDirectory.dir("generated/rustJniLibs")
val buildRustJni by tasks.registering(Exec::class) {
    val repository = rootProject.projectDir.parentFile
    inputs.files(fileTree(File(repository, "crates")) {
        include("**/*.rs", "**/*.c", "**/Cargo.toml")
    })
    inputs.files(File(repository, "Cargo.toml"), File(repository, "Cargo.lock"),
        rootProject.file("scripts/build-rust-android.sh"))
    outputs.dir(generatedJni)
    environment("ANDROID_NDK_HOME", androidComponents.sdkComponents.ndkDirectory.get().asFile.absolutePath)
    commandLine("bash", rootProject.file("scripts/build-rust-android.sh").absolutePath,
        generatedJni.get().asFile.absolutePath)
}
tasks.named("preBuild").configure { dependsOn(buildRustJni) }
