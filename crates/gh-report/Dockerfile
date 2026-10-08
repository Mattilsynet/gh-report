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

# ── Stage 1: Base Chef ──────────────────────────────────────────
FROM rust:1.99.0-bookworm@sha256:114c7a4425406451c2866b6aafe69fe29b1b298832db1277d411ac73c82d04d6 AS chef

COPY --from=lukemathwalker/cargo-chef:0.1.78-rust-1.99.0-bookworm@sha256:2ee6e8edf0b91b5a7295071299596601710bccb243bd9735e0b9abf6e114582b /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef
WORKDIR /app
ENV RUSTUP_TOOLCHAIN=1.99.0

# ── Stage 2: Recipe Planner ──────────────────────────────────────
FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: Builder ─────────────────────────────────────────────
FROM chef AS builder

# cmake + clang + perl: required by aws-lc-sys (transitive dep of jsonwebtoken).
# --no-install-recommends keeps the layer small.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang perl \
 && rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json recipe.json

# Cook dependencies - this layer is cached by Docker until Cargo.lock or dependencies change.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo chef cook --release --locked -p gh-report --recipe-path recipe.json

# Copy workspace source
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

ARG APP_VERSION
ARG APP_GIT_SHA

# Compile application using pre-cooked dependencies
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    APP_VERSION="$APP_VERSION" APP_GIT_SHA="$APP_GIT_SHA" cargo build --release --locked -p gh-report \
 && cp target/release/gh-report /usr/local/bin/gh-report

# ── Stage 4: Runtime ────────────────────────────────────────────
# distroless/cc: glibc + libgcc (required by Rust glibc-linked binaries).
# :nonroot tag runs as uid 65532 — no shell, no package manager.
# Pinned to digest for supply-chain integrity; update digest when upgrading.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f

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

LABEL org.opencontainers.image.source="https://github.com/Mattilsynet/gh-report"
LABEL org.opencontainers.image.description="GitHub organization governance collector and reporter"

# --org is required and must be supplied at deploy time via container args.
# Example: podman run gh-report --org MyOrg
ENTRYPOINT ["/gh-report", "--store-dir", "/home/nonroot/store"]
