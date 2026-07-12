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
FROM rust:1.96.0-bookworm@sha256:19817ead3289c8c631c73df281e18b59b172f6a31f4f563290f69cddd06c30e9 AS builder

# cmake + clang + perl: required by aws-lc-sys (transitive dep of jsonwebtoken).
# --no-install-recommends keeps the layer small.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang perl \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Pin toolchain via env var — the base image ships 1.96.0 but
# without this, rustup may try to re-verify on each cargo invocation.
# rust-toolchain.toml is intentionally excluded from the build context.
ENV RUSTUP_TOOLCHAIN=1.96.0

# Copy workspace manifest and lockfile first for dependency layer caching.
COPY Cargo.toml Cargo.lock ./

# Copy the whole crates/ tree. The root Cargo.toml is a workspace manifest
# listing all 20 members by path, so cargo resolves the entire workspace even
# under `-p gh-report`; every member manifest must therefore be present. Copying
# the full tree is the simple-correct approach (COM-0001) and is robust against
# inter-member manifest references; only gh-report's dependency graph actually
# compiles. gh-report's transitive local crates are cherry-pit-{core,storage,
# web,wq} and pardosa{,-derive,-file,-nats,-schema,-wire}.
COPY crates/ crates/

ARG APP_VERSION

# jemalloc page-return tuning (L1 allocator swap, mission
# l2l1-memcertainty-1783850000): baked in at BUILD time via
# JEMALLOC_SYS_WITH_MALLOC_CONF rather than a runtime env var, so the
# config can never be forgotten in Cloud Run's service env and always
# matches what was verified in this image. background_thread:true turns
# on jemalloc's async purge threads (Linux/pthread only — this is the
# Cloud Run runtime stage, not the QEMU cross-compile note above).
# dirty_decay_ms/muzzy_decay_ms:1000 (1s, well below jemalloc's 10s
# default) is the aggressive-but-safe choice: short enough to actually
# collapse the ~130MB retained-page RSS peak (adr-fmt-iqx9a) between
# collection bursts, long enough that background purge doesn't fire on
# every allocation cycle and compete for CPU under Cloud Run's CPU
# throttling (a 0ms decay purges continuously — the FLO-0008-adjacent
# risk named in adr-fmt-ft0mf §6).
ENV JEMALLOC_SYS_WITH_MALLOC_CONF="background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:1000"

# Cache mounts: Cargo registry + target dir survive across builds.
# The final binary is copied out so the cache mount is not in the image.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    APP_VERSION="$APP_VERSION" cargo build --release -p gh-report \
 && cp target/release/gh-report /usr/local/bin/gh-report

# ── Stage 2: Runtime ────────────────────────────────────────────
# distroless/cc: glibc + libgcc (required by Rust glibc-linked binaries).
# :nonroot tag runs as uid 65532 — no shell, no package manager.
# Pinned to digest for supply-chain integrity; update digest when upgrading.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:b0ae8e989418b458e0f25489bc3be523718938a2b70864cc0f6a00af1ddbd985

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

LABEL org.opencontainers.image.source="https://github.com/acje/solon"
LABEL org.opencontainers.image.description="GitHub organization governance collector and reporter"

# --org is required and must be supplied at deploy time via container args.
# Example: podman run gh-report --org MyOrg
ENTRYPOINT ["/gh-report", "--store-dir", "/home/nonroot/store"]
