#!/usr/bin/env bash
# Android APK of the audio-only client (rr-app), built on macpro.
#   bash ~/RRMumble/build-android-rr.sh [branch]     # default: feature/audio-core
set -uo pipefail
REPO="$HOME/RRMumble"; APP="$REPO/ARMumble"; LOG="$REPO/android-rr-build.log"
BRANCH="${1:-feature/audio-core}"
exec > >(tee "$LOG") 2>&1
echo "ANDROID BUILD START: $(date -u +%Y-%m-%dT%H:%M:%SZ) branch=$BRANCH"
# shellcheck disable=SC1091
source "$REPO/opt/env-android.sh"
export PATH="$JAVA_HOME/bin:$HOME/.nvm/versions/node/v24.21.0/bin:$HOME/.cargo/bin:$PATH"
cd "$APP" || exit 1
git fetch origin --prune || { echo "FETCH_FAILED"; exit 1; }
git checkout -B "$BRANCH" "origin/$BRANCH" || { echo "CHECKOUT_FAILED"; exit 1; }
echo "HEAD: $(git log --oneline -1)"
cd crates/rr-app || exit 1
[ -d ui/node_modules ] || ( cd ui && npm ci ) || { echo "NPM_CI_FAILED"; exit 1; }
if [ ! -d gen/android ]; then
  echo "---- cargo tauri android init ----"
  cargo tauri android init --ci || { echo "ANDROID_INIT_FAILED"; exit 1; }
fi
bash android-overlay/apply.sh || { echo "OVERLAY_FAILED"; exit 1; }
APK_DIR="gen/android/app/build/outputs/apk"
rm -f "$APK_DIR"/universal/release/*.apk 2>/dev/null || true
echo "---- cargo tauri android build ----"
cargo tauri android build --apk --target aarch64 2>&1 | tail -n 60
APK=$(find "$APK_DIR" -name '*.apk' 2>/dev/null | sort | tail -n 1)
if [ -n "$APK" ] && [ -f "$APK" ]; then
  ls -la "$APK"
  # Sign with the local dev keystore (same one the previous APKs used).
  PROPS="$REPO/opt/keys/key.properties"
  [ -f "$PROPS" ] || PROPS="$APP/crates/mumble-tauri/gen/android/key.properties"
  if [ -f "$PROPS" ]; then
    prop() { grep -E "^$1=" "$PROPS" | head -n 1 | cut -d= -f2-; }
    BT=$(ls -d "$ANDROID_HOME"/build-tools/* | sort | tail -n 1)
    OUT="$REPO/RemoteRadioMumble-0.4.0.apk"
    "$BT/zipalign" -f -p 4 "$APK" /tmp/rr-aligned.apk
    "$BT/apksigner" sign --ks "$(prop storeFile)" --ks-key-alias "$(prop keyAlias)" \
      --ks-pass "pass:$(prop storePassword)" --key-pass "pass:$(prop keyPassword)" \
      --out "$OUT" /tmp/rr-aligned.apk
    "$BT/apksigner" verify "$OUT" && echo "SIGNED: $OUT"
    ls -la "$OUT"
  else
    echo "NOTE: no key.properties found; APK is unsigned"
  fi
  echo "ANDROID_RESULT=SUCCESS  $APK"
else
  echo "ANDROID_RESULT=NO_APK"
fi
echo "ANDROID BUILD END: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
