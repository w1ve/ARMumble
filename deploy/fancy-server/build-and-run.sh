#!/usr/bin/env bash
# Build the Fancy Mumble 1.6 server image and run it.
#
# Target: Ubuntu host with Docker already installed. The *container* builds on
# ubuntu:24.04 regardless of the host release, because the fork's
# `cmake_minimum_required(VERSION 3.23...4.2)` is above the CMake 3.22 that
# Ubuntu 22.04 ships. So a 22.04 host is fine — nothing of the host toolchain is
# used.
#
# Usage:
#   ./build-and-run.sh                 # build, then run detached
#   ./build-and-run.sh --build-only    # build and stop
#   ./build-and-run.sh --no-cache      # force a clean rebuild
#   ./build-and-run.sh --shell         # build, then drop into the image
#
# Environment:
#   PORT                       host port to publish  (default 54736)
#   IMAGE                      image tag            (default fancy-mumble:1.6)
#   NAME                       container name       (default fancy-mumble)
#   DATA_DIR                   host data directory  (default ./fancy-mumble-data)
#   MUMBLE_SUPERUSER_PASSWORD  set the SuperUser password on first run
#   MUMBLE_SERVER_PASSWORD     set a server join password
#   MUMBLE_ICE_SECRET_WRITE    Ice write secret (default from the ini)
#   MUMBLE_ICE_ENDPOINT        Ice endpoint override (default loopback :6502)
#
# Extra actions:
#   ./build-and-run.sh --save         # build, then export a portable tarball
#                                      # of the image for use on another host
set -euo pipefail

PORT="${PORT:-54736}"
IMAGE="${IMAGE:-fancy-mumble:1.6}"
NAME="${NAME:-fancy-mumble}"
DATA_DIR="${DATA_DIR:-$PWD/fancy-mumble-data}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BUILD_ONLY=0
SHELL_AFTER=0
SAVE_IMAGE=0
DOCKER_BUILD_ARGS=()

for arg in "$@"; do
    case "$arg" in
        --build-only) BUILD_ONLY=1 ;;
        --shell)      SHELL_AFTER=1 ;;
        --save)       SAVE_IMAGE=1 ;;
        --no-cache)   DOCKER_BUILD_ARGS+=(--no-cache) ;;
        -h|--help)    sed -n '2,33p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *)            echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\n\033[31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

# -- preflight ---------------------------------------------------------------
say "preflight"
command -v docker >/dev/null 2>&1 || die "docker is not installed or not on PATH"

if ! docker info >/dev/null 2>&1; then
    die "the Docker daemon is not reachable.
  If Docker was just installed, your shell may not have picked up the new
  group membership yet. Either log out and back in, or run:  newgrp docker
  Check the daemon with:  systemctl status docker"
fi
echo "docker: $(docker --version)"
echo "host:   $(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME") / $(uname -r)"
echo "cores:  $(nproc)"

for f in Dockerfile mumble-server.ini docker-entrypoint.sh; do
    [[ -f "$SCRIPT_DIR/$f" ]] || die "missing $f next to this script"
done

# -- build -------------------------------------------------------------------
# This compiles Qt6-linked C++ plus a Rust plugin host from source. Expect
# roughly 10-25 minutes on first run depending on core count; the layers cache
# afterwards, so only the final cmake step repeats.
say "building $IMAGE (this takes a while on first run)"
LOG="$SCRIPT_DIR/build-$(date +%Y%m%d-%H%M%S).log"
echo "full build log: $LOG"

if ! docker build "${DOCKER_BUILD_ARGS[@]+"${DOCKER_BUILD_ARGS[@]}"}" \
        -t "$IMAGE" "$SCRIPT_DIR" 2>&1 | tee "$LOG"; then
    echo
    echo "---- last 40 lines of the failure ----"
    tail -40 "$LOG"
    die "build failed; the full log is at $LOG"
fi

say "built $IMAGE"
docker image inspect "$IMAGE" --format '  size: {{.Size}} bytes
  built: {{.Created}}' || true

if [[ "$SHELL_AFTER" == 1 ]]; then
    say "opening a shell in the image (root, for inspection)"
    exec docker run --rm -it --user root --entrypoint /bin/bash "$IMAGE"
fi

# -- portable export for the VPS --------------------------------------------
# `docker save` is the dependency-free way to move an image to a host without a
# registry: it writes a tarball you scp over and `docker load` on the far side.
# gzip because this image is ~580 MB uncompressed. The resulting file is
# self-contained — no build toolchain, no Rust, no Qt dev packages needed on the
# VPS, only Docker and the runtime libraries baked into the image.
if [[ "$SAVE_IMAGE" == 1 ]]; then
    OUT_TAR="$SCRIPT_DIR/fancy-mumble-1.6.tar.gz"
    say "exporting $IMAGE to $OUT_TAR"
    docker save "$IMAGE" | gzip > "$OUT_TAR"
    ls -lh "$OUT_TAR"
    cat <<EOF

$(printf '\033[1m==> deploy on a VPS\033[0m')

  scp $OUT_TAR  user@your-vps:/tmp/
  ssh user@your-vps
    docker load  < /tmp/fancy-mumble-1.6.tar.gz
    # then run it, publishing 54736 and persisting /data:
    docker run -d --name fancy-mumble --restart unless-stopped \\
        --user "\$(id -u):\$(id -g)" \\
        -p 54736:54736/tcp -p 54736:54736/udp \\
        -v /opt/fancy-mumble-data:/data \\
        -e MUMBLE_SUPERUSER_PASSWORD='choose-one' \\
        $IMAGE

  The image was built FROM ubuntu:24.04 but runs anywhere Docker runs,
  including a 22.04 VPS — the host toolchain is irrelevant to a built image.
EOF
fi

[[ "$BUILD_ONLY" == 1 ]] && { say "build-only requested, not starting a container"; exit 0; }

# -- run ---------------------------------------------------------------------
mkdir -p "$DATA_DIR"
# Run as the invoking host user rather than the image's `mumble` account.
#
# The image's own user cannot write a bind-mounted directory owned by someone
# else, and chowning it to that uid needs root — which defeats the point of not
# requiring sudo. Matching the host uid/gid is the standard answer for bind
# mounts: the data directory stays owned by, and editable by, whoever ran this,
# and the container is still unprivileged.
RUN_AS="$(id -u):$(id -g)"
echo "running as uid:gid $RUN_AS so $DATA_DIR stays writable"

if docker ps -a --format '{{.Names}}' | grep -qx "$NAME"; then
    say "removing the previous container named $NAME"
    docker rm -f "$NAME" >/dev/null
fi

say "starting $NAME on port $PORT (tcp+udp)"
# Both protocols: TCP carries control and the audio-tunnel fallback, UDP carries
# the normal low-latency voice path. Publishing only TCP would quietly push
# every client onto the tunnel.
docker run -d \
    --name "$NAME" \
    --restart unless-stopped \
    --user "$RUN_AS" \
    -p "${PORT}:54736/tcp" \
    -p "${PORT}:54736/udp" \
    -v "$DATA_DIR:/data" \
    ${MUMBLE_SUPERUSER_PASSWORD:+-e MUMBLE_SUPERUSER_PASSWORD="$MUMBLE_SUPERUSER_PASSWORD"} \
    ${MUMBLE_SERVER_PASSWORD:+-e MUMBLE_SERVER_PASSWORD="$MUMBLE_SERVER_PASSWORD"} \
    ${MUMBLE_ICE_SECRET_WRITE:+-e MUMBLE_ICE_SECRET_WRITE="$MUMBLE_ICE_SECRET_WRITE"} \
    ${MUMBLE_ICE_ENDPOINT:+-e MUMBLE_ICE_ENDPOINT="$MUMBLE_ICE_ENDPOINT"} \
    "$IMAGE" >/dev/null

sleep 3
say "status"
docker ps --filter "name=$NAME" --format '  {{.Names}}  {{.Status}}  {{.Ports}}'
echo
echo "  recent log:"
docker logs "$NAME" 2>&1 | tail -20 | sed 's/^/    /'

cat <<EOF

$(printf '\033[1m==> next steps\033[0m')

  data dir .......... $DATA_DIR
  config ............ $DATA_DIR/mumble-server.ini
  database .......... $DATA_DIR/mumble-server.sqlite
  logs .............. docker logs -f $NAME
  stop .............. docker stop $NAME
  restart ........... docker restart $NAME

  Connect a Mumble client to  <this-host>:${PORT}

  Verify it is a Fancy server (announces fancy_version) and read back its
  bandwidth budget with the RemoteRadio harness:

    cargo run -p mumble-protocol --features opus-codec --example stereo_live -- \\
        --host <this-host> --port ${PORT} --list-channels

  The SFU is deliberately NOT built (-Dwebrtc-sfu=OFF). That matters: with an
  SFU loaded the server intercepts WebRtcSignal offers and answers them itself,
  which would break RemoteRadio's peer-to-peer design. Pure relay mode is what
  we want.

  Firewall: open ${PORT}/tcp AND ${PORT}/udp.
EOF
