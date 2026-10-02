# Fancy Mumble server (SetZero/mumble-server, branch 1.6.x) — server only.
#
# Every choice below is derived from the fork's own CMake rather than guessed.
# The reasoning is recorded inline because several of these are non-obvious and
# cost real digging.
#
# ---------------------------------------------------------------------------
#  Why ubuntu:24.04 and not 22.04
# ---------------------------------------------------------------------------
# The host is Ubuntu 22.04, but the host's toolchain is irrelevant inside a
# container — and 22.04 cannot build this. The fork's top-level CMakeLists has:
#
#     cmake_minimum_required(VERSION 3.23...4.2)
#
# Ubuntu 22.04 ships CMake 3.22.1, which is below that floor. 24.04 ships 3.28.
# Building in 24.04 sidesteps it with no extra work, and also gives Qt 6.4
# instead of 22.04's 6.2.
# ---------------------------------------------------------------------------

# =============================== builder ==================================
FROM ubuntu:24.04 AS builder

ARG MUMBLE_REPO=https://github.com/SetZero/mumble-server.git
ARG MUMBLE_REF=1.6.x
# Pinned by 3rdparty/mumble-plugin-host/rust-toolchain.toml. The plugin host is
# a Rust crate and MUMBLE_PLUGIN_HOST_LIB is linked PUBLIC into the server
# object library, so cargo is a hard build dependency even with every optional
# Rust module switched off below.
ARG RUST_VERSION=1.95.0

ENV DEBIAN_FRONTEND=noninteractive

# Qt6 Sql + Network + Widgets are all REQUIRED by src/murmur/CMakeLists.txt.
# Widgets is surprising for a headless server but the fork links it
# unconditionally, so it is not optional. BZip2 is REQUIRED; libcap supplies
# CAP_LIBRARY. SQLite driver is what makes the default file-backed database work.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        ninja-build \
        git \
        ca-certificates \
        curl \
        pkg-config \
        python3 \
        qt6-base-dev \
        qt6-base-dev-tools \
        libqt6sql6-sqlite \
        libprotobuf-dev \
        protobuf-compiler \
        libbz2-dev \
        libcap-dev \
        libssl-dev \
        libsqlite3-dev \
        zlib1g-dev \
        # SOCI (3rdparty submodule) unconditionally configures a backend target
        # per database and hard-fails if the headers are absent — it died on
        # `3rdparty/soci/src/backends/mysql/CMakeLists.txt` even though this
        # server only ever uses SQLite. Satisfying every backend is less
        # fragile than fighting SOCI's option names, and costs nothing in the
        # final image: this is the builder stage, which is discarded.
        default-libmysqlclient-dev \
        libpq-dev \
        unixodbc-dev \
        libboost-dev \
        # ZeroC Ice for the RPC interface (-Dice=ON below). libzeroc-ice-dev is
        # the C++ dev package; zeroc-ice-slice provides the .ice definitions the
        # build's slice2cpp step consumes. 3.7.10 on 24.04, well above the
        # CMake-enforced 3.0 floor. Enables live `setConf` (e.g. changing
        # `bandwidth` without a restart) and scripted administration.
        libzeroc-ice-dev \
        zeroc-ice-slice \
        libogg-dev \
        libsndfile1-dev \
        libspeechd-dev \
    && rm -rf /var/lib/apt/lists/*

# Rust for the plugin host. rustup rather than the distro package so the pinned
# 1.95.0 is honoured exactly; rust-toolchain.toml would otherwise try to fetch
# it at build time anyway.
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain "${RUST_VERSION}" --profile minimal \
    && rustc --version && cargo --version

WORKDIR /src
# Shallow but WITH submodules: the build needs twelve of them (CLI11,
# nlohmann_json, soci, spdlog, utfcpp, SPSCQueue, speexdsp, rnnoise,
# cmake-compiler-flags, tracy, flag-icons, minhook). A plain clone fails late,
# during configure, with a confusing missing-target error.
RUN git clone --depth 1 --branch "${MUMBLE_REF}" --recurse-submodules \
        --shallow-submodules "${MUMBLE_REPO}" . \
    && git rev-parse HEAD > /src/GIT_SHA

# ---------------------------------------------------------------------------
#  Upstream bug: standard headers used without being included
# ---------------------------------------------------------------------------
# The fork has a systemic missing-include problem. It compiles wherever
# libstdc++ happens to supply a header transitively and fails where it does
# not, and libstdc++ has been steadily trimming those transitive includes. On
# GCC 13 (Ubuntu 24.04) the build dies twice over:
#
#   PChatPendingKeyRequestsTable.cpp: error: 'std::chrono' has not been declared
#   pchat/ServerBridge.cpp:           error: 'std::views' has not been declared
#
# A scan of src/ found the same latent bug for a dozen headers across ~100
# files — <memory> in 29, <cstdint> in 22, <algorithm> in 13 and so on. Most
# currently compile by luck.
#
# Patching ~100 files with sed would be both noisy and risky: the first
# `#include` in a file is not always at top level, so blind insertion can land
# inside an `#ifdef`. GCC's `-include` does the same job from the command line
# with no source edits, no ordering hazards, and nothing to keep in sync when
# the fork moves. The cost is a slightly larger preprocessor workload per
# translation unit.
#
# Worth reporting upstream to SetZero/mumble-server — the real fix is one
# include line per file.
ENV MUMBLE_FORCED_INCLUDES="-include cstdint -include chrono -include memory \
-include algorithm -include ranges -include optional -include functional \
-include limits -include array -include mutex -include unordered_map \
-include string_view -include numeric -include atomic -include thread"

# ---------------------------------------------------------------------------
#  Upstream breakage: 1.6.x HEAD does not link
# ---------------------------------------------------------------------------
# The newest commit on 1.6.x is "Merge pull request #5 from
# Fancy-Mumble/feat/audit-log-protocol". That PR added five audit-log message
# types to the dispatch table in src/MumbleProtocol.h:
#
#     PROCESS_MUMBLE_TCP_MESSAGE(FancyAuditQuery,        166)
#     PROCESS_MUMBLE_TCP_MESSAGE(FancyAuditResponse,     167)
#     PROCESS_MUMBLE_TCP_MESSAGE(FancyAuditEvent,        168)
#     PROCESS_MUMBLE_TCP_MESSAGE(FancyAuditConfig,       170)
#     PROCESS_MUMBLE_TCP_MESSAGE(FancyAuditConfigUpdate, 171)
#
# Server.h turns that table into member declarations with
#
#     #define PROCESS_MUMBLE_TCP_MESSAGE(name, value) void msg##name(ServerUser *, MumbleProto::name &);
#
# and Server.cpp generates dispatch calls from the same list — but no handler
# bodies were ever written, so the link fails with five undefined references.
# The branch is simply broken as published; this is not a consequence of any
# option chosen above.
#
# No-op bodies are the correct semantics rather than a bodge: an unimplemented
# handler should ignore the message. The fork already uses exactly this shape
# for messages it deliberately does not act on, e.g.
#
#     void Server::msgFancyReadReceiptDeliver(ServerUser *, MumbleProto::FancyReadReceiptDeliver &) {}
#
# Appended to Messages.cpp, which ends at top level with #undef directives and
# no open namespace, so appending is safe.
RUN set -eu; \
    if ! grep -q 'msgFancyAuditConfig' src/murmur/Messages.cpp; then \
        { \
          echo ''; \
          echo '// ---- added by the RemoteRadio container build ----'; \
          echo '// Handlers for the audit-log messages added to the dispatch table by'; \
          echo '// upstream PR #5 without implementations. No-ops: the feature is not'; \
          echo '// used here, and ignoring an unhandled message is the correct behaviour.'; \
          echo 'void Server::msgFancyAuditQuery(ServerUser *, MumbleProto::FancyAuditQuery &) {}'; \
          echo 'void Server::msgFancyAuditResponse(ServerUser *, MumbleProto::FancyAuditResponse &) {}'; \
          echo 'void Server::msgFancyAuditEvent(ServerUser *, MumbleProto::FancyAuditEvent &) {}'; \
          echo 'void Server::msgFancyAuditConfig(ServerUser *, MumbleProto::FancyAuditConfig &) {}'; \
          echo 'void Server::msgFancyAuditConfigUpdate(ServerUser *, MumbleProto::FancyAuditConfigUpdate &) {}'; \
        } >> src/murmur/Messages.cpp; \
        echo 'added five missing FancyAudit* handler definitions'; \
    fi

# -Dserver=ON -Dclient=OFF  : client defaults OFF already; explicit is clearer.
# -Dwebrtc-sfu=OFF          : REQUIRED for RemoteRadio. When an SFU is loaded,
#                             Server::msgWebRtcSignal *intercepts* START/STOP/
#                             SDP_OFFER/ICE_CANDIDATE and answers them itself.
#                             Only the "pure relay mode (no SFU loaded)"
#                             fallback forwards them between peers, which is
#                             what peer-to-peer RemoteRadio needs. Also skips a
#                             large Rust build.
# -Dfcm-push=OFF            : Firebase push module, not wanted here.
# -Dice=ON                  : ZeroC Ice RPC. Enabled per request. Needs
#                             libzeroc-ice-dev + zeroc-ice-slice (above) and the
#                             matching runtime below. Gives live `setConf` (so
#                             `bandwidth` can change without a restart) and
#                             scripted admin. The endpoint and write secret are
#                             set in the ini; by default the endpoint binds
#                             loopback only, so the RPC surface is not exposed
#                             off-host unless you change it.
# -Dplugins=OFF -Dtests=OFF -Doverlay=OFF : client-side or CI-only.
# -Dwarnings-as-errors=OFF  : defaults ON, which is reasonable for the project's
#                             own CI but turns any newer-GCC diagnostic into a
#                             build failure on a distro they did not test.
# -Dlto=OFF                 : defaults ON for Release. Link-time optimisation
#                             roughly doubles link time and buys approximately
#                             nothing for a server whose hot path is a socket
#                             relay, not computation. Also one less place for a
#                             toolchain quirk to surface at link time.
# -Dzeroconf=OFF            : defaults ON and hard-fails with "DNS-SD library
#                             not found!" without avahi-compat headers. It
#                             advertises the server over mDNS on the local
#                             network — meaningless for an internet-facing VPS
#                             reached by hostname, so this is the right answer
#                             rather than a workaround.
# -Ddbus=OFF                : defaults ON. Nothing in a container is going to
#                             talk to a session bus.
# -Dtranslations=OFF        : defaults ON and needs Qt's linguist tools
#                             (lrelease), which are in a package the builder
#                             does not install. A server has no UI to translate.
RUN cmake -G Ninja -B build -S . \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_CXX_FLAGS="${MUMBLE_FORCED_INCLUDES}" \
        -Dserver=ON \
        -Dclient=OFF \
        -Dwebrtc-sfu=OFF \
        -Dfcm-push=OFF \
        -Dice=ON \
        -Dplugins=OFF \
        -Dtests=OFF \
        -Doverlay=OFF \
        -Dwarnings-as-errors=OFF \
        -Dlto=OFF \
        -Dzeroconf=OFF \
        -Ddbus=OFF \
        -Dtranslations=OFF \
    && cmake --build build --parallel "$(nproc)" \
    && find build -name 'mumble-server' -type f -print

# ============================== runtime ===================================
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# Runtime libraries.
#
# Ubuntu 24.04's time_t transition renamed many packages with a `t64` suffix,
# inconsistently enough that hardcoding either spelling is a coin flip. `pick`
# resolves whichever name the archive actually has, so a rename upstream does
# not turn into a failed build to debug.
#
# libmysqlclient and libpq are here because SOCI built its MySQL and PostgreSQL
# backends (the builder stage installs their headers so SOCI's unconditional
# backend configuration succeeds), so the binary links them even though this
# server only ever uses SQLite. Two small packages is a cheaper price than
# fighting SOCI's option names.
RUN set -eu; \
    apt-get update; \
    pick() { \
        for p in "$@"; do \
            if apt-cache show "$p" >/dev/null 2>&1; then echo "$p"; return 0; fi; \
        done; \
        echo "none of these packages exist: $*" >&2; return 1; \
    }; \
    PKGS="$(pick libqt6sql6-sqlite libqt6sql6-sqlite-t64) \
           $(pick libqt6network6 libqt6network6t64) \
           $(pick libqt6widgets6 libqt6widgets6t64) \
           $(pick libqt6xml6 libqt6xml6t64) \
           $(pick libprotobuf32t64 libprotobuf32 libprotobuf-lite32t64) \
           $(pick libmysqlclient21 libmysqlclient21t64) \
           $(pick libpq5 libpq5t64) \
           $(pick libzeroc-ice3.7t64 libzeroc-ice3.7) \
           python3 python3-zeroc-ice \
           libbz2-1.0 libcap2 ca-certificates tzdata"; \
    echo "installing: $PKGS"; \
    apt-get install -y --no-install-recommends $PKGS; \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/build/mumble-server /usr/local/bin/mumble-server
COPY --from=builder /src/GIT_SHA /usr/local/share/mumble-server.gitsha

# The plugin host is a Rust cdylib built by cargo during the build, not an apt
# package — the server links it (MUMBLE_PLUGIN_HOST_LIB) so it has to travel
# into the runtime image with the binary. ldconfig registers /usr/local/lib so
# the dynamic loader finds it without an LD_LIBRARY_PATH override.
COPY --from=builder /src/3rdparty/mumble-plugin-host/target/release/libmumble_plugin_host.so \
     /usr/local/lib/libmumble_plugin_host.so
RUN ldconfig

# Fail at IMAGE BUILD time rather than at first run if the runtime stage is
# missing a shared library. Getting this wrong is otherwise a crash loop that
# looks like a config problem.
RUN ldd /usr/local/bin/mumble-server > /tmp/ldd.txt 2>&1; \
    if grep -q 'not found' /tmp/ldd.txt; then \
        echo "FATAL: unresolved shared libraries in the runtime image:"; \
        grep 'not found' /tmp/ldd.txt; \
        exit 1; \
    fi; \
    echo "all shared libraries resolved"

COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
COPY mumble-server.ini /etc/mumble/mumble-server.ini.default
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Virtual-server administration over Ice, used by `docker exec`. The Python Ice
# client (python3-zeroc-ice, installed above) loads the slice at runtime, so the
# slice definition travels into the image with it. These let an operator create
# and configure virtual servers without an Ice toolchain on the host.
COPY --from=builder /src/src/murmur/MumbleServer.ice /usr/local/share/MumbleServer.ice
COPY mumble-ice-admin.py /usr/local/lib/mumble-ice-admin.py
COPY create-virtual-servers.sh /usr/local/bin/create-virtual-servers.sh
RUN chmod +x /usr/local/bin/create-virtual-servers.sh /usr/local/lib/mumble-ice-admin.py \
    # The slice pulls in `#include <Ice/SliceChecksumDict.ice>`, a system slice
    # the Debian python3-zeroc-ice package does not ship (only its pre-generated
    # Python), and installing the full zeroc-ice-all-dev just for it would drag a
    # compiler toolchain into the runtime image. The admin client never calls
    # getSliceChecksums(), the one method that uses the type, so strip BOTH the
    # include and that method declaration — removing only the include leaves a
    # dangling `Ice::SliceChecksumDict` reference that fails slice parsing.
    && sed -i '/#include <Ice\/SliceChecksumDict.ice>/d; /Ice::SliceChecksumDict getSliceChecksums/d' \
        /usr/local/share/MumbleServer.ice

# Unprivileged. The server only needs a privileged port if you configure one,
# and 54736 is not privileged.
RUN useradd --system --create-home --home-dir /var/lib/mumble --shell /usr/sbin/nologin mumble \
    && mkdir -p /data && chown mumble:mumble /data

# The server links Qt6 Widgets, so a Qt platform plugin gets resolved at
# startup even though nothing is drawn. `offscreen` avoids it hunting for an X
# or Wayland display that a container will never have.
ENV QT_QPA_PLATFORM=offscreen

VOLUME ["/data"]

# Mumble needs BOTH protocols on every port: TCP for control and the audio
# tunnel fallback, UDP for the normal low-latency voice path. The documented
# range covers up to 100 virtual servers (base 54735 + server number 1..100 =
# 54736..54835), with 54836 left as headroom. EXPOSE is documentation only;
# actual reachability comes from the run command — `--network host` for the
# range, or `-p` for a single server. See deploy README.
EXPOSE 54736-54836/tcp 54736-54836/udp

USER mumble
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
