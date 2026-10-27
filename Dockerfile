# Bifrost appliance image: Bifrost + a pinned Archipelago +
# bubblewrap, everything mutable under one volume.
#
#   podman build -t bifrost .            (or docker build)
#   podman run -d --name bifrost --stop-timeout 30 --security-opt unmask=/proc/* \
#       -p 8080:8080 -p 38281-38300:38281-38300 -v bifrost-data:/var/lib/bifrost bifrost
#   podman logs bifrost                  # first-run /setup link
#
# --security-opt unmask=/proc/* (docker: --security-opt systempaths=unconfined)
# lets bubblewrap mount its own /proc, which the strong sandbox tier needs;
# without it Bifrost runs on the limits-only tier and refuses custom apworlds.
# --stop-timeout: each AP server gets up to 15 s to save on shutdown.
#
# Build args: AP_VERSION (a release tag of Bifrost's Archipelago fork,
# packaging/archipelago/), AP_REPO.

ARG DEBIAN=trixie
# apt's privilege drop to _apt (uid/gid 65534) fails under rootless builders
# whose subordinate ID range doesn't reach 65534 (e.g. inside an unprivileged
# LXC); building is root-in-namespace anyway.
ARG APT="apt-get -o APT::Sandbox::User=root"

# ---- Bifrost ----
FROM docker.io/library/rust:1.98-${DEBIAN} AS bifrost
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY src src
COPY static static
RUN cargo build --release --locked --bin bifrost

# ---- Archipelago (source checkout + venv; paths match the runtime stage) ----
FROM docker.io/library/debian:${DEBIAN}-slim AS archipelago
ARG APT
ARG AP_VERSION=0.6.8-bifrost.2
ARG AP_REPO=https://github.com/vidarrio/Archipelago
RUN ${APT} update && ${APT} install -y --no-install-recommends \
        ca-certificates git python3 python3-venv python3-dev build-essential \
    && rm -rf /var/lib/apt/lists/*
RUN git clone --depth 1 --branch "${AP_VERSION}" "${AP_REPO}" /opt/archipelago \
    && rm -rf /opt/archipelago/.git
WORKDIR /opt/archipelago
# Core + every bundled world's requirements (what AP's ModuleUpdate would
# install on first run — done here because the install is read-only at
# runtime). setuptools<81: some worlds still import pkg_resources.
RUN python3 -m venv venv \
    && venv/bin/pip install --no-cache-dir --upgrade pip "setuptools<81" wheel cython \
    && venv/bin/pip install --no-cache-dir -r requirements.txt \
    && for r in worlds/*/requirements.txt; do \
         venv/bin/pip install --no-cache-dir -r "$r" || echo "WARN: $r failed"; \
       done
# Prebuild AP's Cython speedups (otherwise it compiles them at runtime, which
# needs a compiler and a writable install).
RUN venv/bin/cythonize -b -i _speedups.pyx && rm -rf build

# ---- runtime ----
FROM docker.io/library/debian:${DEBIAN}-slim
ARG APT
# python3-venv + git: building the venvs of releases installed from the
# settings page (some worlds pin requirements to git commits); gcc +
# python3-dev: their Cython speedups and any requirement without a wheel.
RUN ${APT} update && ${APT} install -y --no-install-recommends \
        ca-certificates python3 python3-venv python3-dev gcc libc6-dev git bubblewrap tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 900 --home-dir /var/lib/bifrost --shell /usr/sbin/nologin bifrost \
    && install -d -o bifrost -g bifrost /var/lib/bifrost
COPY --from=archipelago /opt/archipelago /opt/archipelago
COPY --from=bifrost /src/target/release/bifrost /usr/local/bin/bifrost
COPY scripts /opt/bifrost/scripts
COPY vendor /opt/bifrost/vendor

# Defaults spelled out (they match Bifrost's built-in ones).
ENV BIFROST_DATA=/var/lib/bifrost \
    BIFROST_AP_DIR=/opt/archipelago \
    BIFROST_AP_PYTHON=/opt/archipelago/venv/bin/python \
    BIFROST_RUNTIME_DIR=/opt/bifrost \
    BIFROST_PORT=8080 \
    BIFROST_AP_PORTS=38281-38300

USER bifrost
VOLUME /var/lib/bifrost
EXPOSE 8080 38281-38300
STOPSIGNAL SIGTERM
# tini reaps orphans; Bifrost itself stops the AP servers (saving) on SIGTERM.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/bifrost"]
