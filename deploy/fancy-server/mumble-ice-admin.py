#!/usr/bin/env python3
"""Minimal Ice admin client for the Fancy Mumble server.

Drives the Meta/Server Ice interface to create and configure virtual servers.
Runs inside the container (the Ice endpoint is loopback-only), using the
``python3-zeroc-ice`` package and the slice definition shipped at
``/usr/local/share/MumbleServer.ice``.

Only the operations needed for multi-server bring-up are implemented:
``newServer``, ``start``, per-server ``setConf``, and ``getBootConf``/``setConf``
for the boot flag. Everything else is left to the full Ice tooling.

The write secret is required for any mutating call; the server throws
``InvalidSecretException`` without it, which this surfaces as a clear error.
"""

import argparse
import sys

import Ice

SLICE = "/usr/local/share/MumbleServer.ice"
# Loopback inside the container, matching the ini's `ice=` endpoint.
DEFAULT_PROXY = "Meta:tcp -h 127.0.0.1 -p 6502"


def connect(secret: str):
    """Return (communicator, Meta proxy) with the write secret in the context."""
    props = Ice.createProperties()
    # The server puts the secret in an implicit request context, not the proxy.
    props.setProperty("Ice.ImplicitContext", "Shared")
    init = Ice.InitializationData()
    init.properties = props
    communicator = Ice.initialize(init)
    communicator.getImplicitContext().put("secret", secret)

    Ice.loadSlice("--all " + SLICE)
    import MumbleServer  # noqa: E402  (generated after loadSlice)

    base = communicator.stringToProxy(DEFAULT_PROXY)
    meta = MumbleServer.MetaPrx.checkedCast(base)
    if meta is None:
        raise SystemExit("could not reach the Meta object at " + DEFAULT_PROXY)
    return communicator, meta, MumbleServer


def cmd_create(args) -> int:
    """Ensure `count` virtual servers exist, all booted and set to boot.

    `newServer()` returns the lowest unoccupied id, so on a database holding
    ids 0..k-1 it returns k. We call it until the total count is reached rather
    than targeting specific ids, which keeps the ids dense from 0 and matches the
    base-port arithmetic (server id N listens on base + N).
    """
    communicator, meta, _ = connect(args.secret)
    try:
        existing = sorted(s.id() for s in meta.getAllServers())
        created = []
        # Create until we have at least `count` servers.
        while len(existing) < args.count:
            srv = meta.newServer()
            created.append(srv.id())
            existing = sorted(s.id() for s in meta.getAllServers())

        # Boot flag + start, for every server at or above `start`.
        started = []
        for s in meta.getAllServers():
            if s.id() < args.start:
                continue
            s.setConf("boot", "true")
            if not s.isRunning():
                s.start()
                started.append(s.id())

        print("created ids:", created or "none")
        print("started ids:", started or "none")
        print("all server ids now:", sorted(s.id() for s in meta.getAllServers()))
        return 0
    finally:
        communicator.destroy()


def cmd_setconf(args) -> int:
    communicator, meta, _ = connect(args.secret)
    try:
        srv = meta.getServer(args.server)
        if srv is None:
            raise SystemExit(f"no virtual server with id {args.server}")
        srv.setConf(args.key, args.value)
        print(f"server {args.server}: {args.key} = {args.value}")
        return 0
    finally:
        communicator.destroy()


def cmd_list(args) -> int:
    communicator, meta, _ = connect(args.secret)
    try:
        for s in meta.getAllServers():
            sid = s.id()
            running = s.isRunning()
            # Effective port is base + id unless a per-server override is set.
            override = s.getConf("port")
            print(f"  server {sid}  running={running}  port_override={override or '(default: base+id)'}")
        return 0
    finally:
        communicator.destroy()


def main() -> int:
    # `--secret` is attached to each subcommand rather than the top level so it
    # can appear after the subcommand name — argparse requires top-level options
    # *before* the subcommand, which is an easy and confusing mistake to make
    # from a wrapper script. A shared parent parser keeps it defined once.
    secret = argparse.ArgumentParser(add_help=False)
    secret.add_argument("--secret", required=True, help="Ice write secret")

    p = argparse.ArgumentParser(description="Fancy Mumble Ice admin")
    sub = p.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("create", parents=[secret],
                       help="ensure N virtual servers exist and run")
    c.add_argument("--count", type=int, required=True)
    c.add_argument("--start", type=int, default=0)
    c.set_defaults(func=cmd_create)

    s = sub.add_parser("setconf", parents=[secret],
                       help="set a per-server config key")
    s.add_argument("--server", type=int, required=True)
    s.add_argument("--key", required=True)
    s.add_argument("--value", required=True)
    s.set_defaults(func=cmd_setconf)

    l = sub.add_parser("list", parents=[secret], help="list virtual servers")
    l.set_defaults(func=cmd_list)

    args = p.parse_args()
    try:
        return args.func(args)
    except Ice.Exception as e:
        print(f"Ice error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
