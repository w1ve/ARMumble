#!/usr/bin/env bash
# Find the ZeroC Ice packages available on ubuntu:24.04, both the C++ dev
# package (build) and its runtime, so the Dockerfile names them correctly rather
# than by guess. Ice has been packaged under several names over the years
# (zeroc-ice, libzeroc-ice-dev, libzeroc-ice3.7-dev), so resolve it live.
set -u
docker run --rm ubuntu:24.04 bash -c '
    set -e
    apt-get update -qq >/dev/null 2>&1
    echo "=== dev candidates ==="
    apt-cache search . 2>/dev/null | grep -iE "zeroc|libice.*dev|ice.*slice" | sort
    echo "=== package policy for likely names ==="
    for p in zeroc-ice-all-dev libzeroc-ice-dev zeroc-ice-slice libzeroc-ice3.7 libzeroc-ice3.7-dev; do
        v="$(apt-cache policy "$p" 2>/dev/null | awk "/Candidate:/{print \$2}")"
        printf "  %-26s %s\n" "$p" "${v:-<not found>}"
    done
'
