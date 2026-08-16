# syntax=docker/dockerfile:1
# Multi-stage build for dnshub.
#
# Builder stage: uses the Rust toolchain to compile a release binary.
# Runtime stage: minimal debian:bookworm-slim with the C libraries needed
# by heed (LMDB) and rusqlite (SQLite) — both are dynamically linked.

ARG RUST_IMAGE=rust:1.95-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim

# ---------------------------------------------------------------------------
# Builder stage
# ---------------------------------------------------------------------------
FROM ${RUST_IMAGE} AS builder

# Install build-time C dependencies for LMDB (liblmdb-dev) and SQLite
# (rusqlite uses the "bundled" feature, but keep pkg-config available).
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        liblmdb-dev \
        libsqlite3-dev \
        pkg-config \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Pre-copy manifests to leverage Docker layer caching of the dependency
# build. Cargo will download and compile dependencies only when the
# manifests change.
COPY Cargo.toml Cargo.lock ./

# Create a stub source tree so `cargo build` can resolve the workspace
# without failing on missing files. The real source is copied afterwards
# and the binary is rebuilt.
RUN mkdir -p src && \
    echo 'fn main() {}' > src/main.rs && \
    echo '' > src/lib.rs

# Build dependencies only (release). The dummy main.rs produces a throwaway
# binary; the real build happens after the source is copied.
RUN cargo build --release || true

# Copy the real source tree.
COPY src/ src/

# Rebuild with the real source. touch ensures cargo sees the updated files.
RUN touch src/main.rs src/lib.rs && cargo build --release

# Stage the release binary.
RUN cp target/release/dnshub /usr/local/bin/dnshub

# ---------------------------------------------------------------------------
# Runtime stage
# ---------------------------------------------------------------------------
FROM ${RUNTIME_IMAGE} AS runtime

# Install runtime C libraries required by the dynamically-linked native
# dependencies:
#   - liblmdb0     : LMDB (heed crate)
#   - libsqlite3-0 : SQLite (rusqlite crate, even with "bundled" it links
#                    the system libm/libpthread; keep it for safety)
#   - ca-certificates : TLS verification for blocklist downloads (reqwest)
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        liblmdb0 \
        libsqlite3-0 \
        ca-certificates \
        tini \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for the runtime.
RUN groupadd --system dnshub \
    && useradd --system --gid dnshub --home-dir /var/lib/dnshub --shell /usr/sbin/nologin dnshub

# Configuration and data directories. The config dir is mounted from the
# host (Ansible-managed); the data dir is a named volume for LMDB/SQLite.
RUN mkdir -p /etc/dnshub /var/lib/dnshub /var/lib/dnshub/blocklists \
    && chown -R dnshub:dnshub /var/lib/dnshub /etc/dnshub

# Copy the release binary from the builder stage.
COPY --from=builder /usr/local/bin/dnshub /usr/local/bin/dnshub

USER dnshub

# Ports exposed by dnshub (see PRD section 6.1):
#   53   - DNS (UDP/TCP)
#   853  - DNS-over-TLS
#   443  - DNS-over-HTTPS
#   67   - DHCPv4 (UDP, requires macvlan for L2 broadcast access)
#   547  - DHCPv6 (UDP, requires macvlan for L2 broadcast access)
#   69   - TFTP (UDP, PXE/BOOTP network boot)
#   9090 - Prometheus metrics
#   8080 - NextJS frontend + REST API
EXPOSE 53/udp 53/tcp 853/tcp 443/tcp 67/udp 547/udp 69/udp 9090/tcp 8080/tcp

# tini handles signal forwarding and zombie reaping for the tokio runtime.
ENTRYPOINT ["/usr/bin/tini", "--"]

# Default config path; can be overridden via env or command args.
CMD ["dnshub", "--config", "/etc/dnshub/dnshub.toml"]
