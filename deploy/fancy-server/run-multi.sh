#!/usr/bin/env bash
# Run the Fancy Mumble container hosting MANY virtual servers across a port
# range, with the host firewall opened for that range.
#
# One mumble-server process hosts all virtual servers; it is not one container
# per server. Each virtual server N listens on (base_port + N) on both TCP and
# UDP — the server computes this in Server::readParams() as
# `usPort = Meta::mp->usPort + iServerNum`. With the shipped ini base of 54735,
# server 1 is 54736 ... server 100 is 54835.
#
# Why --network host rather than -p for a range:
#   `-p 54736-54836:54736-54836/tcp -p .../udp` creates 101 x 2 = 202 docker-proxy
#   forwarding rules, which is slow to start and wastes a process per port. Host
#   networking shares the host's stack directly, so every port the server binds
#   is immediately reachable with zero per-port overhead. The cost is that the
#   container is not network-isolated from the host — acceptable for a dedicated
#   VoIP box, and the norm for Mumble servers hosting many instances.
#
# Usage (run as a user who can reach docker, e.g. root on the VPS):
#   ./run-multi.sh
#
# Environment:
#   IMAGE        image tag           (default fancy-mumble:1.6)
#   NAME         container name      (default fancy-mumble)
#   DATA_DIR     host data directory (default /opt/fancy-mumble-data)
#   PORT_LOW     firewall range low  (default 54736)
#   PORT_HIGH    firewall range high (default 54836)
#   MUMBLE_SUPERUSER_PASSWORD   set on first run
#   MUMBLE_ICE_SECRET_WRITE     Ice write secret (needed to create servers)
#
# Firewall is NOT touched by this script — open the ports you actually use with
# ufw yourself, e.g.  ufw allow 54736:54836/tcp && ufw allow 54736:54836/udp
# (and remember a cloud security group is separate from the host firewall).
set -euo pipefail

IMAGE="${IMAGE:-fancy-mumble:1.6}"
NAME="${NAME:-fancy-mumble}"
DATA_DIR="${DATA_DIR:-/opt/fancy-mumble-data}"
PORT_LOW="${PORT_LOW:-54736}"
PORT_HIGH="${PORT_HIGH:-54836}"

say() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\n\033[31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

command -v docker >/dev/null 2>&1 || die "docker not found"
docker info >/dev/null 2>&1 || die "cannot reach the Docker daemon (need root or the docker group)"
docker image inspect "$IMAGE" >/dev/null 2>&1 \
    || die "image $IMAGE not present. docker load < fancy-mumble-1.6.tar.gz first."

# -- data dir ----------------------------------------------------------------
# No --user here (unlike the single-server script): host networking plus a
# root-run container is the normal VPS case, so let the image's own `mumble`
# user own the data. Make the directory writable by it. The container's uid is
# read from the image rather than assumed.
mkdir -p "$DATA_DIR"
MUMBLE_UID="$(docker run --rm --entrypoint id "$IMAGE" -u 2>/dev/null || echo '')"
if [[ -n "$MUMBLE_UID" ]]; then
    chown -R "$MUMBLE_UID:$MUMBLE_UID" "$DATA_DIR" 2>/dev/null \
        || { say "could not chown $DATA_DIR; making it world-writable instead"; chmod 777 "$DATA_DIR"; }
fi

# -- run ---------------------------------------------------------------------
if docker ps -a --format '{{.Names}}' | grep -qx "$NAME"; then
    say "removing the previous container named $NAME"
    docker rm -f "$NAME" >/dev/null
fi

say "starting $NAME with host networking (serves the whole ${PORT_LOW}-${PORT_HIGH} range)"
docker run -d \
    --name "$NAME" \
    --restart unless-stopped \
    --network host \
    -v "$DATA_DIR:/data" \
    ${MUMBLE_SUPERUSER_PASSWORD:+-e MUMBLE_SUPERUSER_PASSWORD="$MUMBLE_SUPERUSER_PASSWORD"} \
    ${MUMBLE_ICE_SECRET_WRITE:+-e MUMBLE_ICE_SECRET_WRITE="$MUMBLE_ICE_SECRET_WRITE"} \
    "$IMAGE" >/dev/null

sleep 3
say "status"
docker ps --filter "name=$NAME" --format '  {{.Names}}  {{.Status}}'
echo "  recent log:"
docker logs "$NAME" 2>&1 | tail -12 | sed 's/^/    /'

cat <<EOF

$(printf '\033[1m==> virtual servers\033[0m')

  A fresh database has only server 0 (port ${PORT_LOW} — the first id is 0, not
  1). To ensure N servers exist, create them over Ice; they persist in the
  database and boot on restart. e.g. for 10 servers (ids 0..9):

    docker exec -e MUMBLE_ICE_SECRET_WRITE=... $NAME \\
        /usr/local/bin/create-virtual-servers.sh 10

  Server id N listens on ${PORT_LOW} + N automatically (server 0 -> ${PORT_LOW},
  server 1 -> $(( PORT_LOW + 1 )), ...). Set per-server overrides (name,
  password, bandwidth, even a custom port) with:

    docker exec -e MUMBLE_ICE_SECRET_WRITE=... $NAME \\
        python3 /usr/local/lib/mumble-ice-admin.py --secret ... \\
        setconf --server N --key welcometext --value 'Hello'

  The Ice endpoint is loopback inside the container; run admin via 'docker exec'.
EOF
