plugins {
    id("com.android.application")
}

android {
    namespace = "net.lego.hahmannsreach"
    compileSdk = 35
    defaultConfig {
        applicationId = "net.lego.hahmannsreach"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1"
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            // sideload build: signed with the debug key so it installs without a keystore
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    packaging {
        jniLibs.useLegacyPackaging = true
    }
    // native libs come from cargo-ndk (build-apk.sh) into src/main/jniLibs
}
