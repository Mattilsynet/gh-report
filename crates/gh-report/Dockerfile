# gh-report — GCP Cloud Run optimised container image
#
# Multi-stage build: compile on Debian bookworm, run on distroless.
# Priorities: security → performance → simplicity.
#
# Build context must be the workspace root (sibling crates are required):
#   podman build -f crates/gh-report/Dockerfile -t gh-report .
#
# Cross-compile to linux/amd64 requires a CI builder or Cloud Build —
# QEMU emulation crashes Rust's jemalloc allocator (SIGSEGV in
# rustc -vV). See OPERATIONS.md for details.
#
# Run:
#   podman run --rm -p 8080:8080 -e GITHUB_TOKEN=ghp_... -e BIND_ADDRESS=0.0.0.0 gh-report --org MyOrg

# ── Stage 1: Build ───────────────────────────────────────────────
FROM rust:1.95.0-bookworm@sha256:225aa827d55fae9816a0492284592827e794a5247c6c6a961c3b471b344295ec AS builder

# cmake + clang + perl: required by aws-lc-sys (transitive dep of jsonwebtoken).
# --no-install-recommends keeps the layer small.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang perl \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Pin toolchain via env var — the base image ships 1.95.0 but
# without this, rustup may try to re-verify on each cargo invocation.
# rust-toolchain.toml is intentionally excluded from the build context.
ENV RUSTUP_TOOLCHAIN=1.95.0

# Copy workspace manifest and lockfile first for dependency layer caching.
COPY Cargo.toml Cargo.lock ./

# Copy all crate manifests (needed by cargo to resolve the workspace).
COPY crates/gh-report/Cargo.toml    crates/gh-report/Cargo.toml
COPY crates/quics-memoization/Cargo.toml crates/quics-memoization/Cargo.toml
COPY crates/quics-aggregate/Cargo.toml crates/quics-aggregate/Cargo.toml

# Copy all crate sources.
COPY crates/gh-report/src/          crates/gh-report/src/
COPY crates/gh-report/templates/    crates/gh-report/templates/
COPY crates/quics-memoization/src/  crates/quics-memoization/src/
COPY crates/quics-aggregate/src/      crates/quics-aggregate/src/

# Cache mounts: Cargo registry + target dir survive across builds.
# The final binary is copied out so the cache mount is not in the image.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p gh-report \
 && cp target/release/gh-report /usr/local/bin/gh-report

# ── Stage 2: Runtime ────────────────────────────────────────────
# distroless/cc: glibc + libgcc (required by Rust glibc-linked binaries).
# :nonroot tag runs as uid 65534 — no shell, no package manager.
# Pinned to digest for supply-chain integrity; update digest when upgrading.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:4589ac94334222262b0cc69d1b481fe23dfb2aa2637598236747a2443011dace

COPY --from=builder --chmod=0555 /usr/local/bin/gh-report /gh-report

# /home/nonroot is writable by the nonroot user.
# The app creates store/ subdirectory at startup.
WORKDIR /home/nonroot
USER nonroot

HEALTHCHECK NONE
EXPOSE 8080

# Structured JSON logging for Cloud Logging.
ENV GH_REPORT_LOG_FORMAT=json
# Bind to all interfaces (container networking requires this).
ENV BIND_ADDRESS=0.0.0.0

LABEL org.opencontainers.image.source="https://github.com/acje/quicksilver"
LABEL org.opencontainers.image.description="GitHub organization governance collector and reporter"

# --org is required and must be supplied at deploy time via container args.
# Example: podman run gh-report --org MyOrg
ENTRYPOINT ["/gh-report", "--store-dir", "/home/nonroot/store"]
