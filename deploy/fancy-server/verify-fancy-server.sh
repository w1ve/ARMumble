#!/usr/bin/env bash
# Verify the freshly built Fancy Mumble container with the stereo harness.
#
# Runs on big-linux against 127.0.0.1 because that host sits behind a cloudflared
# proxy, so its container port is not reachable from the Windows workstation.
# Testing from the same machine also keeps the network out of the measurement.
set -u
cd /home/gerry/RRMumble/ARMumble || exit 1
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:/home/linuxbrew/.linuxbrew/bin:$PATH"

H=./target/debug/examples/stereo_live
TARGET_HOST=127.0.0.1
TARGET_PORT=54736
OUT=/home/gerry/fancy-server/verify.txt
: > "$OUT"

{
    echo "=============================================================="
    echo " 1. identity and bandwidth budget"
    echo "=============================================================="
    "$H" --host "$TARGET_HOST" --port "$TARGET_PORT" --list-channels

    echo
    echo "=============================================================="
    echo " 2. sustained stereo: 450 frames (9 s) at 128 kbps"
    echo "    At the old 72000 budget this delivered exactly 360 frames —"
    echo "    the 360-slot BandwidthRecord ring — then started dropping."
    echo "    With bandwidth=192000 all 450 should arrive."
    echo "=============================================================="
    "$H" --host "$TARGET_HOST" --port "$TARGET_PORT" --channel-id 0 \
         --signal tones --bitrate 128000 --frames 450
} >> "$OUT" 2>&1

grep -E 'server version|Fancy|stock Mumble|max_bandwidth|largest Opus|requested|id  |received|byte-identical|separation|rms|PASS|FAIL|note:|^  [0-9]' "$OUT"
echo "VERIFY_DONE"
