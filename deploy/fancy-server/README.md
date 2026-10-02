# Fancy Mumble 1.6 server — Docker build

Builds [`SetZero/mumble-server`](https://github.com/SetZero/mumble-server)
(branch `1.6.x`) into a server-only Docker image, configured for RemoteRadio.

RemoteRadio's peer-to-peer screen-sharing signalling rides on the Fancy-only
`WebRtcSignal` message (wire ID 120), which a **stock** Mumble server silently
drops. So the remote-desktop half of RemoteRadio needs this server. Voice and
stereo audio work on stock Mumble; only the signalling needs the fork. See
`../../doc/remote-radio/CONTEXT.md` §16 for the full reasoning.

## Build and run

```bash
./build-and-run.sh                      # build, then run on port 54736
MUMBLE_SUPERUSER_PASSWORD=... ./build-and-run.sh   # set SuperUser on first run
./build-and-run.sh --build-only         # build only
./build-and-run.sh --save               # also export a portable tarball
./build-and-run.sh --no-cache           # clean rebuild
```

Requires Docker. Builds `FROM ubuntu:24.04` regardless of the host release —
the fork needs CMake >= 3.23, which Ubuntu 22.04 does not ship, but a built
image runs fine on a 22.04 host.

## Deploy on a VPS without a build toolchain

```bash
./build-and-run.sh --save               # produces fancy-mumble-1.6.tar.gz
scp fancy-mumble-1.6.tar.gz user@vps:/tmp/
ssh user@vps
  docker load < /tmp/fancy-mumble-1.6.tar.gz
  docker run -d --name fancy-mumble --restart unless-stopped \
      --user "$(id -u):$(id -g)" \
      -p 54736:54736/tcp -p 54736:54736/udp \
      -v /opt/fancy-mumble-data:/data \
      -e MUMBLE_SUPERUSER_PASSWORD='choose-one' \
      fancy-mumble:1.6
```

Open **54736/tcp and 54736/udp** at the firewall — both are required (TCP for
control and the audio tunnel, UDP for low-latency voice).

## Configuration

`mumble-server.ini` is seeded into `/data` on first run and never overwritten,
so edits survive rebuilds. Key settings and their rationale are commented in the
file. The most important is `bandwidth=192000`: Mumble enforces this via a
rolling window and the default 72000 silently throttles sustained stereo. Env
overrides handled by the entrypoint: `MUMBLE_PORT`, `MUMBLE_BANDWIDTH`,
`MUMBLE_SERVER_PASSWORD`, `MUMBLE_SUPERUSER_PASSWORD` (first run only),
`MUMBLE_ICE_ENDPOINT`, `MUMBLE_ICE_SECRET_WRITE`.

Built with the WebRTC SFU **disabled** on purpose: with an SFU loaded the server
answers `WebRtcSignal` offers itself instead of relaying them peer to peer,
which breaks RemoteRadio. Ice RPC is **enabled** (loopback endpoint) for live
reconfiguration and scripted admin.

## Upstream patches applied at build time

The fork's `1.6.x` HEAD does not build or link cleanly on Ubuntu 24.04 / GCC 13.
The Dockerfile works around two genuine upstream bugs — missing standard-library
`#include`s across ~100 files, and five `FancyAudit*` message handlers that are
dispatched but never defined (PR #5) — plus the usual dependency and
feature-flag wrangling. Each workaround is documented inline in the `Dockerfile`.
Both bugs are worth reporting to the fork.

## Helper scripts

- `verify-fancy-server.sh` — point the RemoteRadio `stereo_live` harness at the
  running container: confirms it announces `fancy_version`, reports its bandwidth
  budget, and relays sustained stereo byte-for-byte.
- `sustain.sh` — one 9-second 128 kbps stereo run, the quick smoke test.
- `find-ice-pkg.sh` — resolves the ZeroC Ice package names on the target Ubuntu.
