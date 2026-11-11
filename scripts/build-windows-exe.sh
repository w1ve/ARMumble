#!/usr/bin/env bash
# Single-file Windows client EXE (static C runtime), cross-built on macpro.
#
#   bash ~/RRMumble/build-windows-exe.sh [branch]      # default: feature/windows-static-crt
#
# Output: ~/RRMumble/ARMumble/target/x86_64-pc-windows-msvc/release/rrmumble.exe
# The script FAILS the build if the exe still imports the VC/UCRT runtime DLLs,
# i.e. if it would show "VCRUNTIME140.dll was not found" on a clean PC.
set -uo pipefail
REPO="$HOME/RRMumble"
APP="$REPO/ARMumble"
LOG="$REPO/windows-exe-build.log"
BRANCH="${1:-feature/windows-static-crt}"
TRIPLE=x86_64-pc-windows-msvc
exec > >(tee "$LOG") 2>&1
echo "========================================================"
echo "WINDOWS EXE BUILD START: $(date -u +%Y-%m-%dT%H:%M:%SZ)  branch=$BRANCH"
echo "========================================================"

# cargo-xwin + clang-cl + lld-link toolchain. SKIP_SIGNAL_BRIDGE=1 comes from here
# (same as the local Windows builds: no signal-bridge DLL beside the exe).
# shellcheck disable=SC1091
source "$REPO/opt/env-windows.sh" >/dev/null 2>&1
# env-windows.sh turns LTO off and uses 16 codegen units for quick test builds.
# A shipping exe uses the workspace [profile.release] (thin LTO, 1 codegen unit),
# exactly like the local Windows builds.
unset CARGO_PROFILE_RELEASE_LTO CARGO_PROFILE_RELEASE_CODEGEN_UNITS
# libopus: audiopus_sys's own CMake build hard-codes /MD (old CMake policy), which
# makes the exe import the UCRT DLLs and fails to link with crt-static. Use a copy
# pre-built with /MT instead (opt/opus-win-mt, built with opt/cmake/xwin-static-crt.cmake).
# audiopus_sys appends /lib itself, so this is the install prefix.
export OPUS_LIB_DIR="$REPO/opt/opus-win-mt"
export OPUS_STATIC=1
if ! grep -aqi 'defaultlib:libcmt' "$OPUS_LIB_DIR/lib/opus.lib" 2>/dev/null; then
  echo "ABORT: $OPUS_LIB_DIR/lib/opus.lib missing or not built with /MT"; exit 1
fi
export PATH="$HOME/.nvm/versions/node/v24.21.0/bin:$HOME/.cargo/bin:$PATH"

cd "$APP" || exit 1
git fetch origin --prune || { echo "FETCH_FAILED"; exit 1; }
git checkout -B "$BRANCH" "origin/$BRANCH" || { echo "CHECKOUT_FAILED"; exit 1; }
echo "HEAD: $(git log --oneline -1)"
if ! grep -q 'crt-static' .cargo/config.toml; then
  echo "ABORT: crt-static is not in .cargo/config.toml on this branch"; exit 1
fi

# The Dockerized .deb build runs as root and leaves root-owned crates/*/gen
# (tauri-build's generated ACL schemas) behind; the build script then dies with
# "Permission denied (os error 13)". We cannot delete root-owned dirs without
# sudo, but we own the parent, so rename them aside (same-parent rename works)
# and let tauri-build regenerate them. Only the generated app-client/app-station
# dirs are touched, never the tracked Android project in crates/mumble-tauri/gen.
for g in crates/rr-app/gen; do
  [ -d "$g" ] || continue
  if [ -n "$(find "$g" -not -user "$(id -un)" -print -quit 2>/dev/null)" ]; then
    aside="$g.root-owned.$(date +%s)"
    mv "$g" "$aside" && echo "NOTE: $g was root-owned; renamed to $aside (safe to 'sudo rm -rf' later)"
  fi
done

# The UI is built by tauri's beforeBuildCommand (npm run build in ui/).
if [ ! -d crates/rr-app/ui/node_modules ]; then
  echo "---- npm ci (no node_modules) ----"
  ( cd crates/rr-app/ui && npm ci ) || { echo "NPM_CI_FAILED"; exit 1; }
fi

echo "---- cargo tauri build (app-client, no bundle) via cargo-xwin ----"
cd crates/rr-app || exit 1
rm -f "$APP/target/$TRIPLE/release/rrmumble.exe"
cargo tauri build --runner cargo-xwin --target "$TRIPLE" --no-bundle
RC=$?
echo "TAURI_RC=$RC"
if [ "$RC" != "0" ]; then echo "WINDOWS_EXE_RESULT=TAURI_BUILD_FAILED"; exit 1; fi

EXE="$APP/target/$TRIPLE/release/rrmumble.exe"
if [ ! -f "$EXE" ]; then echo "WINDOWS_EXE_RESULT=NO_EXE"; exit 1; fi
ls -la "$EXE"
file "$EXE" 2>/dev/null | cut -c1-160

echo "---- DLLs the exe imports (load-time + delay-load) ----"
llvm-readobj --coff-imports "$EXE" | grep -E '^\s+Name: .*\.[dD][lL][lL]' | sed 's/^ *Name: //' | sort -fu > /tmp/rr_exe_imports.txt
cat /tmp/rr_exe_imports.txt
echo "---- count: $(wc -l < /tmp/rr_exe_imports.txt) ----"
if grep -iE '^(vcruntime|msvcp|vcomp|concrt|ucrtbase|api-ms-win-crt)' /tmp/rr_exe_imports.txt; then
  echo "CRT_STATIC_RESULT=FAIL  (still imports the C runtime; see names above)"
  echo "WINDOWS_EXE_RESULT=CRT_NOT_STATIC"
  exit 1
fi
echo "CRT_STATIC_RESULT=OK  (no VC++/UCRT runtime DLLs imported)"
echo "WINDOWS_EXE_RESULT=SUCCESS"
echo "WINDOWS EXE BUILD END: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
