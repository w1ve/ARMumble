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
./build-and-run.sh                      # build, then run a single server on 54736
MUMBLE_SUPERUSER_PASSWORD=... ./build-and-run.sh   # set SuperUser on first run
./build-and-run.sh --build-only         # build only
./build-and-run.sh --save               # also export a portable tarball
./build-and-run.sh --no-cache           # clean rebuild
```

Requires Docker. Builds `FROM ubuntu:24.04` regardless of the host release —
the fork needs CMake >= 3.23, which Ubuntu 22.04 does not ship, but a built
image runs fine on a 22.04 host.

## Multiple virtual servers across a port range

One `mumble-server` process hosts many virtual servers; it is **not** one
container per server. Each virtual server with id `N` listens on `base_port + N`
on both TCP and UDP, where `base_port` is the ini's `port` (54736 here). The
**first server has id 0**, so:

| server id | port |
|---|---|
| 0 | 54736 |
| 1 | 54737 |
| … | … |
| 100 | 54836 |

Run with host networking so the whole range is reachable without 200+
per-port forwarding rules:

```bash
MUMBLE_SUPERUSER_PASSWORD=... MUMBLE_ICE_SECRET_WRITE=... ./run-multi.sh
```

A fresh database has only server 0. Create more over Ice — they persist in the
database and boot on restart:

```bash
# ensure 10 virtual servers exist (ids 0..9 -> ports 54736..54745)
docker exec -e MUMBLE_ICE_SECRET_WRITE=yoursecret fancy-mumble \
    /usr/local/bin/create-virtual-servers.sh 10

# list them
docker exec -e MUMBLE_ICE_SECRET_WRITE=yoursecret fancy-mumble \
    python3 /usr/local/lib/mumble-ice-admin.py list --secret yoursecret

# per-server config (name, password, bandwidth, even a custom port)
docker exec -e MUMBLE_ICE_SECRET_WRITE=yoursecret fancy-mumble \
    python3 /usr/local/lib/mumble-ice-admin.py setconf \
    --server 3 --key registername --value 'My Net' --secret yoursecret
```

Open the ports you use at the firewall yourself — the scripts do not touch it:

```bash
ufw allow 54736:54836/tcp
ufw allow 54736:54836/udp
```

With host networking the container installs no iptables rules, so ufw applies
normally (unlike Docker `-p` publishing, which bypasses it). A cloud security
group, if any, is separate and must also allow the range.

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
