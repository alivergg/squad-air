import java.io.File

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

fun resolveVersionName(): String {
    val raw = System.getenv("SQUADAIR_VERSION")?.trim()
    return if (!raw.isNullOrEmpty()) raw else "1.0.0"
}

fun resolveVersionCode(versionName: String): Int {
    val parts = versionName.split('.').mapNotNull { it.filter(Char::isDigit).toIntOrNull() }
    return when {
        parts.size >= 3 -> parts[0] * 10000 + parts[1] * 100 + parts[2]
        parts.size == 2 -> parts[0] * 10000 + parts[1] * 100
        parts.size == 1 -> parts[0] * 10000
        else -> 10000
    }.coerceAtLeast(1)
}

val appVersionName = resolveVersionName()
val appVersionCode = resolveVersionCode(appVersionName)

android {
    namespace = "app.squadair.mobile"
    compileSdk = 34

    defaultConfig {
        applicationId = "app.squadair.mobile"
        minSdk = 26
        targetSdk = 34
        versionCode = appVersionCode
        versionName = appVersionName

        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    signingConfigs {
        create("release") {
            val envPath = System.getenv("ANDROID_KEYSTORE_PATH")?.trim()
            val envStorePass = System.getenv("ANDROID_KEYSTORE_PASSWORD")?.trim()
            val envAlias = System.getenv("ANDROID_KEY_ALIAS")?.trim()
            val envKeyPass = System.getenv("ANDROID_KEY_PASSWORD")?.trim()

            if (!envPath.isNullOrEmpty() &&
                !envStorePass.isNullOrEmpty() &&
                !envAlias.isNullOrEmpty() &&
                !envKeyPass.isNullOrEmpty() &&
                File(envPath).exists()
            ) {
                storeFile = File(envPath)
                storePassword = envStorePass
                keyAlias = envAlias
                keyPassword = envKeyPass
            } else {
                val fallbackDir = layout.buildDirectory.dir("keystore").get().asFile
                fallbackDir.mkdirs()
                val fallbackStore = File(fallbackDir, "fallback-release.keystore")
                val fallbackPass = "squadair-release-fallback"
                val fallbackAlias = "squadair"
                if (!fallbackStore.exists()) {
                    val keytoolBin = File(System.getProperty("java.home"), "bin/keytool").absolutePath
                    val proc = ProcessBuilder(
                        keytoolBin,
                        "-genkeypair",
                        "-v",
                        "-keystore",
                        fallbackStore.absolutePath,
                        "-storetype",
                        "PKCS12",
                        "-alias",
                        fallbackAlias,
                        "-keyalg",
                        "RSA",
                        "-keysize",
                        "2048",
                        "-validity",
                        "10000",
                        "-storepass",
                        fallbackPass,
                        "-keypass",
                        fallbackPass,
                        "-dname",
                        "CN=SquadAir, OU=Mobile, O=SquadAir, L=Internet, S=Net, C=US",
                    ).redirectErrorStream(true).start()
                    val output = proc.inputStream.bufferedReader().readText()
                    val code = proc.waitFor()
                    if (code != 0) {
                        throw GradleException("Failed to generate fallback keystore: $output")
                    }
                }
                storeFile = fallbackStore
                storePassword = fallbackPass
                keyAlias = fallbackAlias
                keyPassword = fallbackPass
            }
            enableV1Signing = true
            enableV2Signing = true
            enableV3Signing = true
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            isShrinkResources = false
            signingConfig = signingConfigs.getByName("release")
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
        debug {
            isMinifyEnabled = false
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
            useLegacyPackaging = true
        }
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

val buildRustNative = tasks.register("buildRustNative") {
    group = "rust"
    description = "Compiles libsquad_core.so for arm64-v8a, copies libc++_shared.so, and generates UniFFI bindings"

    doLast {
        val repoRoot = rootProject.projectDir.parentFile
        val ndkHome = System.getenv("ANDROID_NDK_HOME")
            ?: throw GradleException("ANDROID_NDK_HOME is not set")
        val jniOutDir = file("src/main/jniLibs")
        val arm64OutDir = File(jniOutDir, "arm64-v8a")
        arm64OutDir.mkdirs()

        val cargoEnv = mapOf(
            "CMAKE_TOOLCHAIN_FILE" to "$ndkHome/build/cmake/android.toolchain.cmake",
            "ANDROID_ABI" to "arm64-v8a",
            "ANDROID_PLATFORM" to "android-26",
            "OPUS_STATIC" to "1",
        )

        exec {
            workingDir = repoRoot
            environment(cargoEnv)
            commandLine(
                "cargo",
                "ndk",
                "-t",
                "arm64-v8a",
                "-p",
                "26",
                "-o",
                jniOutDir.absolutePath,
                "build",
                "--release",
                "--locked",
                "-p",
                "squad-core",
                "--no-default-features",
                "--features",
                "android-ffi",
            )
        }

        val hostTags = listOf("linux-x86_64", "darwin-arm64", "darwin-x86_64")
        val libcxx = hostTags
            .map { tag ->
                File("$ndkHome/toolchains/llvm/prebuilt/$tag/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so")
            }
            .firstOrNull { it.exists() }
            ?: throw GradleException("Could not locate libc++_shared.so in $ndkHome")
        libcxx.copyTo(File(arm64OutDir, "libc++_shared.so"), overwrite = true)

        val builtSo = File(repoRoot, "target/aarch64-linux-android/release/libsquad_core.so")
        exec {
            workingDir = repoRoot
            commandLine(
                "cargo",
                "run",
                "--locked",
                "-p",
                "squad-core",
                "--no-default-features",
                "--features",
                "android-ffi",
                "--bin",
                "uniffi-bindgen",
                "--",
                "generate",
                "--library",
                builtSo.absolutePath,
                "--language",
                "kotlin",
                "--out-dir",
                file("src/main/java").absolutePath,
            )
        }
    }
}

val runRustBuildProp = providers.gradleProperty("runRustBuild").orNull == "true"
if (runRustBuildProp || !file("src/main/jniLibs/arm64-v8a/libsquad_core.so").exists()) {
    tasks.matching { it.name == "preBuild" }.configureEach {
        dependsOn(buildRustNative)
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.09.03")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.9.2")
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.6")
    implementation("androidx.media:media:1.7.0")
    implementation("net.java.dev.jna:jna:5.14.0@aar")
}
