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

# Mirror the repo layout: `categorizer` include_str!s `../../../prompts/`,
# so `prompts/` must sit next to the workspace, not inside it.
WORKDIR /build/finreport-rs

COPY prompts/ /build/prompts/
COPY finreport-rs/ ./

# BuildKit cache mounts: cargo registry/git and the workspace target dir are
# persisted across CI runs on the same daemon, so incremental builds reuse
# downloaded crates and compiled dependencies. Each mount has an explicit `id`
# scoped to this project and builder image: without one, BuildKit keys the
# cache by path alone, so any other image on the daemon mounting /build/target
# shares it — and build scripts compiled there against a newer glibc then fail
# here ("GLIBC_2.39 not found"). Bump the id when changing the builder image.
#
# Every binary the deploy runbook needs against the running image lives here:
# `webapp`/`import-transactions` are the two long-running services;
# `projector` is `docker-compose.yml`'s `finreport-be-projector` entrypoint;
# `legacy-backfill` and `user-admin` are one-off `docker run --entrypoint`
# steps in the runbook; `migration` (a separate workspace package, not a
# `webapp` bin — it's the `sea-orm-migration` CLI the `migration` crate
# already ships) is the explicit migrate step `APP_run_migrations=false`
# requires. `fixture-replay` rides along too: same package, same deps already
# compiled, so adding it costs nothing extra. `labeler` is compose's
# `finreport-be-labeler` entrypoint; `category-seed` loads the taxonomy once.
RUN --mount=type=cache,id=finreport-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=finreport-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=finreport-target-rust1.93-bookworm,target=/build/finreport-rs/target \
    cargo build --release \
        --package webapp \
        --bin webapp \
        --bin import-transactions \
        --bin projector \
        --bin legacy-backfill \
        --bin user-admin \
        --bin fixture-replay \
        --bin labeler \
        --bin category-seed \
        --package migration \
        --bin migration && \
    cp /build/finreport-rs/target/release/webapp            /usr/local/bin/finreport-be && \
    cp /build/finreport-rs/target/release/import-transactions /usr/local/bin/finreport-be-importer && \
    cp /build/finreport-rs/target/release/projector          /usr/local/bin/projector && \
    cp /build/finreport-rs/target/release/legacy-backfill    /usr/local/bin/legacy-backfill && \
    cp /build/finreport-rs/target/release/user-admin         /usr/local/bin/user-admin && \
    cp /build/finreport-rs/target/release/fixture-replay     /usr/local/bin/fixture-replay && \
    cp /build/finreport-rs/target/release/labeler      /usr/local/bin/labeler && \
    cp /build/finreport-rs/target/release/category-seed /usr/local/bin/category-seed && \
    cp /build/finreport-rs/target/release/migration          /usr/local/bin/finreport-be-migrate

# ── Runtime stage ─────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/finreport-be          /usr/local/bin/finreport-be
COPY --from=builder /usr/local/bin/finreport-be-importer /usr/local/bin/finreport-be-importer
COPY --from=builder /usr/local/bin/projector             /usr/local/bin/projector
COPY --from=builder /usr/local/bin/legacy-backfill        /usr/local/bin/legacy-backfill
COPY --from=builder /usr/local/bin/user-admin             /usr/local/bin/user-admin
COPY --from=builder /usr/local/bin/fixture-replay         /usr/local/bin/fixture-replay
COPY --from=builder /usr/local/bin/labeler               /usr/local/bin/labeler
COPY --from=builder /usr/local/bin/category-seed         /usr/local/bin/category-seed
COPY --from=builder /usr/local/bin/finreport-be-migrate   /usr/local/bin/finreport-be-migrate

# webapp reads `../assets/*` relative to its cwd — mirror the source layout
# so the relative paths resolve inside the container.
WORKDIR /app/webapp
COPY assets/  /app/assets/
COPY prompts/ /app/prompts/

EXPOSE 8080

ENTRYPOINT ["finreport-be"]