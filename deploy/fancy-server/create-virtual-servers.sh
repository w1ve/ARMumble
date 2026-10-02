#!/usr/bin/env bash
# Create virtual Mumble servers over Ice, inside the container.
#
# Run with `docker exec`, because the Ice endpoint is bound to loopback inside
# the container and is not reachable from outside:
#
#   docker exec -e MUMBLE_ICE_SECRET_WRITE=yoursecret fancy-mumble \
#       /usr/local/bin/create-virtual-servers.sh <count> [start_at]
#
# <count>     total number of virtual servers to ensure exist (idempotent:
#             only the missing ones are created).
# [start_at]  lowest server id to ensure exists (default 0 — the first server
#             has id 0, which maps to the base port).
#
# Each created server inherits the ini defaults, including bandwidth=192000, and
# auto-listens on (base_port + its id): server 0 -> 54736, server 1 -> 54737, …
# It is also marked to boot on restart, so the set survives a container restart.
#
# This is a thin wrapper around the Python Ice client; all the real work is
# there so the logic is testable and legible.
set -euo pipefail

COUNT="${1:?usage: create-virtual-servers.sh <count> [start_at]}"
START="${2:-0}"
: "${MUMBLE_ICE_SECRET_WRITE:?set MUMBLE_ICE_SECRET_WRITE (the Ice write secret)}"

exec python3 /usr/local/lib/mumble-ice-admin.py create \
    --count "$COUNT" --start "$START" \
    --secret "$MUMBLE_ICE_SECRET_WRITE"
