# syntax=docker/dockerfile:1.4

# ── Build stage ────────────────────────────────────────────────────────
FROM rust:1.93-slim-bookworm AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY finreport-rs/ ./

# BuildKit cache mounts: cargo registry/git and the workspace target dir are
# persisted across CI runs on the same daemon, so incremental builds reuse
# downloaded crates and compiled dependencies.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --package webapp --bin webapp && \
    cp /build/target/release/webapp /usr/local/bin/finreport-be

# ── Runtime stage ─────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/finreport-be /usr/local/bin/finreport-be

# webapp reads `../assets/*` relative to its cwd — mirror the source layout
# so the relative paths resolve inside the container.
WORKDIR /app/webapp
COPY assets/  /app/assets/
COPY prompts/ /app/prompts/

EXPOSE 8080

ENTRYPOINT ["finreport-be"]