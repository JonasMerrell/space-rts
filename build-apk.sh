#!/bin/bash
# Build the Android APK: Rust client (cargo-ndk) + Gradle packaging. Output: ./Kepler.apk
#   ./build-apk.sh            release build, arm64-v8a + x86_64 (phone + emulator)
#   ABIS="arm64-v8a" ./build-apk.sh   phone only (smaller)
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/android-sdk}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/25.2.9519653}"
export JAVA_HOME="${JAVA_HOME:-/usr/lib64/jvm/java-17-openjdk-17}"
ABIS="${ABIS:-arm64-v8a x86_64}"
OUT="${OUT:-Kepler.apk}"

APP=android/app/src/main
rm -rf "$APP/jniLibs"
args=()
for a in $ABIS; do args+=(-t "$a"); done
cargo ndk "${args[@]}" -P 26 -o "$APP/jniLibs" build --release -p client --lib

# game assets (macroquad reads them from the APK's assets/ dir; the UI loads "assets/...")
rm -rf "$APP/assets"
mkdir -p "$APP/assets/assets"
cp -r assets/sprites assets/audio assets/manifest.json "$APP/assets/assets/"

# launcher icons
for d in mdpi hdpi xhdpi xxhdpi xxxhdpi; do
  mkdir -p "$APP/res/mipmap-$d"
  cp "assets/icon/ic_launcher_$d.png" "$APP/res/mipmap-$d/ic_launcher.png"
done

(cd android && ./gradlew -q :app:assembleRelease --no-daemon)
cp android/app/build/outputs/apk/release/app-release.apk "$OUT"
echo "APK: $(du -h "$OUT" | cut -f1)  $(pwd)/$OUT"
