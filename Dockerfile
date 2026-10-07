# syntax=docker/dockerfile:1.4

# ── Build stage ────────────────────────────────────────────────────────
FROM rust:1.93-slim-bookworm AS builder

# make/g++ are for librdkafka, which rdkafka-sys builds from source with
# librdkafka's own configure script (mklove). Do NOT switch that to the crate's
# `cmake-build` feature: the cmake path never passes WITH_CURL=OFF, so
# librdkafka's default turns CURL on and the build then needs libcurl headers.
# The configure path explicitly disables curl/ssl/gssapi/zlib/zstd, so no extra
# dev packages are required and the runtime stage needs nothing new either.
RUN apt-get update && apt-get install -y \
        pkg-config libssl-dev make g++ \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY finreport-rs/ ./

# BuildKit cache mounts: cargo registry/git and the workspace target dir are
# persisted across CI runs on the same daemon, so incremental builds reuse
# downloaded crates and compiled dependencies.
#
# Every binary the deploy runbook needs against the running image lives here:
# `webapp`/`import-transactions` are the two long-running services;
# `projector` is `docker-compose.yml`'s `finreport-be-projector` entrypoint;
# `legacy-backfill` and `user-admin` are one-off `docker run --entrypoint`
# steps in the runbook; `migration` (a separate workspace package, not a
# `webapp` bin — it's the `sea-orm-migration` CLI the `migration` crate
# already ships) is the explicit migrate step `APP_run_migrations=false`
# requires. `fixture-replay` rides along too: same package, same deps already
# compiled, so adding it costs nothing extra.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release \
        --package webapp \
        --bin webapp \
        --bin import-transactions \
        --bin projector \
        --bin legacy-backfill \
        --bin user-admin \
        --bin fixture-replay \
        --package migration \
        --bin migration && \
    cp /build/target/release/webapp            /usr/local/bin/finreport-be && \
    cp /build/target/release/import-transactions /usr/local/bin/finreport-be-importer && \
    cp /build/target/release/projector          /usr/local/bin/projector && \
    cp /build/target/release/legacy-backfill    /usr/local/bin/legacy-backfill && \
    cp /build/target/release/user-admin         /usr/local/bin/user-admin && \
    cp /build/target/release/fixture-replay     /usr/local/bin/fixture-replay && \
    cp /build/target/release/migration          /usr/local/bin/finreport-be-migrate

# ── Runtime stage ─────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/finreport-be          /usr/local/bin/finreport-be
COPY --from=builder /usr/local/bin/finreport-be-importer /usr/local/bin/finreport-be-importer
COPY --from=builder /usr/local/bin/projector             /usr/local/bin/projector
COPY --from=builder /usr/local/bin/legacy-backfill        /usr/local/bin/legacy-backfill
COPY --from=builder /usr/local/bin/user-admin             /usr/local/bin/user-admin
COPY --from=builder /usr/local/bin/fixture-replay         /usr/local/bin/fixture-replay
COPY --from=builder /usr/local/bin/finreport-be-migrate   /usr/local/bin/finreport-be-migrate

# webapp reads `../assets/*` relative to its cwd — mirror the source layout
# so the relative paths resolve inside the container.
WORKDIR /app/webapp
COPY assets/  /app/assets/
COPY prompts/ /app/prompts/

EXPOSE 8080

ENTRYPOINT ["finreport-be"]