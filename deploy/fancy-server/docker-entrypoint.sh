#!/usr/bin/env bash
# Entrypoint for the Fancy Mumble server container.
#
# Keeps the image stateless and the data directory authoritative: configuration
# and database both live on the mounted volume, and this script only seeds them
# on first run.
set -euo pipefail

INI=/data/mumble-server.ini

log() { printf '[entrypoint] %s\n' "$*"; }

# -- first run: seed the config ---------------------------------------------
# Never overwrite an existing ini. An image rebuild must not silently revert
# someone's tuning, least of all `bandwidth`, which is the setting that decides
# whether stereo survives.
if [[ ! -f "$INI" ]]; then
    log "no $INI yet, seeding from the image default"
    cp /etc/mumble/mumble-server.ini.default "$INI"
else
    log "using existing $INI"
fi

# -- optional overrides from the environment --------------------------------
# Edit in place so the file on the volume stays the single source of truth,
# rather than passing flags the operator cannot later see.
set_ini() {
    local key="$1" value="$2"
    if grep -qE "^${key}=" "$INI"; then
        # `|` as the delimiter: values may contain slashes.
        sed -i "s|^${key}=.*|${key}=${value}|" "$INI"
    else
        printf '%s=%s\n' "$key" "$value" >> "$INI"
    fi
    log "set ${key}"
}

[[ -n "${MUMBLE_PORT:-}" ]]            && set_ini port "$MUMBLE_PORT"
[[ -n "${MUMBLE_BANDWIDTH:-}" ]]       && set_ini bandwidth "$MUMBLE_BANDWIDTH"
[[ -n "${MUMBLE_SERVER_PASSWORD:-}" ]] && set_ini serverpassword "$MUMBLE_SERVER_PASSWORD"
[[ -n "${MUMBLE_WELCOMETEXT:-}" ]]     && set_ini welcometext "$MUMBLE_WELCOMETEXT"
[[ -n "${MUMBLE_USERS:-}" ]]           && set_ini users "$MUMBLE_USERS"
[[ -n "${MUMBLE_ICE_ENDPOINT:-}" ]]    && set_ini ice "$MUMBLE_ICE_ENDPOINT"
[[ -n "${MUMBLE_ICE_SECRET_WRITE:-}" ]] && set_ini icesecretwrite "$MUMBLE_ICE_SECRET_WRITE"

# -- SuperUser password ------------------------------------------------------
# `--read-su-pw` takes the virtual-server number and reads the password from
# stdin, which is the only scriptable route; `--set-su-pw` prompts interactively
# and would hang a container. Runs as a separate short-lived invocation before
# the real one, exactly as the upstream Docker image does.
#
# ⚠️ ONLY on first run, keyed off the database not yet existing. An early version
# applied this on every start, which meant any later run that happened to pass a
# different value (e.g. a throwaway during image export) silently reset the
# SuperUser password. The database is the right sentinel because the password
# lives in it; MUMBLE_SUPERUSER_PASSWORD on a subsequent start is now ignored.
DB_FILE="$(grep -E '^database=' "$INI" | cut -d= -f2-)"
if [[ -n "${MUMBLE_SUPERUSER_PASSWORD:-}" ]]; then
    if [[ -n "$DB_FILE" && -f "$DB_FILE" ]]; then
        log "database already exists; ignoring MUMBLE_SUPERUSER_PASSWORD (set once, on first run)"
    else
        log "first run: setting SuperUser password on virtual server 1"
        printf '%s' "$MUMBLE_SUPERUSER_PASSWORD" \
            | mumble-server -i "$INI" --read-su-pw 1 || {
                log "WARNING: could not set the SuperUser password (continuing)"
            }
    fi
fi

# -- report what we are about to run ----------------------------------------
if [[ -f /usr/local/share/mumble-server.gitsha ]]; then
    log "build: $(cat /usr/local/share/mumble-server.gitsha)"
fi
log "port=$(grep -E '^port=' "$INI" | cut -d= -f2) bandwidth=$(grep -E '^bandwidth=' "$INI" | cut -d= -f2)"

# `--foreground` is the negation of `--detach` in the CLI11 flag group
# (`--detach,!--no-detach,--background,!--foreground`). A detached server would
# exit immediately and take the container with it.
#
# `exec` so the server becomes PID 1 and receives SIGTERM directly, giving it a
# clean shutdown on `docker stop` instead of being killed after the timeout.
log "starting mumble-server in the foreground"
exec mumble-server -i "$INI" --foreground "$@"
