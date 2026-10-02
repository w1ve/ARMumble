#!/usr/bin/env bash
# One clean sustained-stereo run against the local container, for the final
# verification before committing.
set -u
cd /home/gerry/RRMumble/ARMumble || exit 1
./target/debug/examples/stereo_live \
    --host 127.0.0.1 --port 54736 --channel-id 0 \
    --signal tones --bitrate 128000 --frames 450 > /tmp/sustain.txt 2>&1
echo "harness exit=$?"
sed -n '/\[6\] relay/,$ p' /tmp/sustain.txt
