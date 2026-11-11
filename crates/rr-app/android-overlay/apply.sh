#!/usr/bin/env bash
# Apply the RemoteRadio Android customisations to the generated Gradle project.
#
#   cd crates/rr-app && cargo tauri android init     # once, creates gen/android
#   bash android-overlay/apply.sh                    # idempotent
#
# Adds the microphone / foreground-service permissions, the AudioService, and
# replaces MainActivity with the version that requests permissions at launch.
set -euo pipefail
cd "$(dirname "$0")/.."
ANDROID=gen/android
[ -d "$ANDROID" ] || { echo "run 'cargo tauri android init' first"; exit 1; }
PKG_DIR="$ANDROID/app/src/main/java/com/remoteradio/mumble"
MANIFEST="$ANDROID/app/src/main/AndroidManifest.xml"
mkdir -p "$PKG_DIR"
cp android-overlay/MainActivity.kt android-overlay/AudioService.kt "$PKG_DIR/"

add_permission() {
  grep -q "android.permission.$1" "$MANIFEST" && return 0
  sed -i "0,/<manifest[^>]*>/s//&\n    <uses-permission android:name=\"android.permission.$1\" \/>/" "$MANIFEST"
}
for p in INTERNET RECORD_AUDIO MODIFY_AUDIO_SETTINGS FOREGROUND_SERVICE FOREGROUND_SERVICE_MICROPHONE \
         FOREGROUND_SERVICE_MEDIA_PLAYBACK POST_NOTIFICATIONS WAKE_LOCK; do
  add_permission "$p"
done

if ! grep -q "AudioService" "$MANIFEST"; then
  sed -i 's#</application>#    <service android:name=".AudioService" android:foregroundServiceType="microphone|mediaPlayback" android:exported="false" />\n    </application>#' "$MANIFEST"
fi
echo "Android overlay applied."