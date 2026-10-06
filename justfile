# finreport — Rust workspace + Docker (base + main image) + Portainer deploy
#
# Local dev override: BUILD_TOOLS_DIR=/path/to/build-tools just _bootstrap

set allow-duplicate-variables
set allow-duplicate-recipes

build_tools_dir   := ".build/build-tools"
docker_image_name := "finreport-be"

# Local-dev config for every recipe that talks to the `dev-up` stack, so no
# finreport-rs/.env (or 1Password) is needed to run the demo. Each value can
# still be overridden from the calling shell. These are throwaway localhost
# defaults, never real secrets; real credentials only come in via .env.tpl.
local_env := 'APP_database_url="${APP_database_url:-postgresql://finreport:${POSTGRES_PASSWORD:-finreport}@127.0.0.1:${FINREPORT_PG_PORT:-5432}/finreport}" APP_kafka_brokers="${APP_kafka_brokers:-127.0.0.1:${FINREPORT_KAFKA_PORT:-19092}}" APP_cookie_secure="${APP_cookie_secure:-false}" APP_allowed_origins="${APP_allowed_origins:-http://localhost:5173}"'

import? '.build/build-tools/common.just'

[private]
default: _bootstrap
    @just --list

[private]
_bootstrap:
    #!/usr/bin/env bash
    set -e
    if [ ! -e {{build_tools_dir}} ]; then
        mkdir -p .build
        if [ -n "${BUILD_TOOLS_DIR:-}" ]; then
            echo "==> Symlinking local build-tools: $BUILD_TOOLS_DIR"
            ln -s "$BUILD_TOOLS_DIR" {{build_tools_dir}}
        else
            echo "==> Cloning build-tools..."
            git clone --depth=1 https://gitea.lab.anydef.de/homelab/build-tools.git {{build_tools_dir}}
        fi
    fi

# Run Rust unit tests across the workspace. Fast, offline — the `integration`
# feature (testcontainers-backed) is never enabled here; see `test-integration`.
test:
    cargo test --manifest-path finreport-rs/Cargo.toml

# Run the testcontainers-backed integration suite (§8): projector replay
# determinism, precedence, backfill idempotency, real-schema GraphQL. Needs a
# working local Docker daemon; pulls Postgres + Kafka images on first run.
# Gated behind the `integration` feature so `just test` stays fast and
# offline.
test-integration:
    cargo test --manifest-path finreport-rs/Cargo.toml -p webapp --features integration

# Rust lint used by CI (§9 WP6): clippy across the workspace, including test
# targets, with warnings as errors — but only warnings a *clean* run produces
# today. Pre-existing dead_code on the WP0 bin stubs (projector/fixture-replay/
# user-admin/legacy-backfill all still `unimplemented!()`) is expected and
# intentionally not promoted to -D warnings; fixing that is each stub's own
# WP, not WP6's. `cargo fmt --check` is deliberately not run here: the repo
# predates a repo-wide rustfmt pass, so enforcing it now would fail CI on
# unrelated pre-existing files rather than anything this change touched.
lint:
    cargo clippy --manifest-path finreport-rs/Cargo.toml --workspace --all-targets

# Start local Postgres (via compose) in the background.
# No secrets needed here — POSTGRES_PASSWORD defaults in docker-compose.local.yml.
db-up:
    docker compose -f docker-compose.local.yml up finreport-be-postgres -d --wait

# Stop the local Postgres started by `db-up`.
db-down:
    docker compose -f docker-compose.local.yml down

# Start the local dev stack: Postgres + single-node Redpanda + the four
# ingest/watermark topics (same partitions/cleanup policies as
# terraform/kafka/main.tf — keep both in step). No Comdirect credentials and
# no network access to the bank or kafka.lab.anydef.de needed from here on.
#
# Ports default to 5432/19092 but are overridable (shared-machine friendly):
#     FINREPORT_PG_PORT=15432 FINREPORT_KAFKA_PORT=29092 just dev-up
dev-up:
    docker compose -f docker-compose.local.yml \
        up finreport-be-postgres finreport-redpanda finreport-redpanda-init -d --wait

# Stop everything `dev-up` started (Postgres volume included).
dev-down:
    docker compose -f docker-compose.local.yml down -v

# Run the projector locally (§2.3) against the stack from `dev-up`: consumes
# the ingest topics and builds the read model in the local Postgres. Pass
# `--until-caught-up` to exit at the log end instead of tailing it (what
# `dev-demo` uses); omit it to keep tailing like the deployed service.
#
# Config comes from `local_env` (top of this file); no .env needed.
dev-projector *ARGS:
    cd finreport-rs && \
        {{local_env}} RUST_LOG=info \
        cargo run -p webapp --bin projector -- {{ARGS}}

# Create (or no-op onto) the `dev` user with a known password, so the seeded
# stack has something to log in with. Password from $FINREPORT_PASSWORD,
# defaulting to `dev` for local use only — never set that default outside
# this recipe.
seed-user:
    cd finreport-rs && \
        {{local_env}} FINREPORT_PASSWORD="${FINREPORT_PASSWORD:-dev}" \
        cargo run -p webapp --bin user-admin -- create-user --username dev --display-name "Local Dev"

# Publish the WP0 fixture corpus (finreport-rs/webapp/fixtures) onto the
# ingest topics, so `dev-projector` has something to build the read model
# from without any Comdirect credentials.
seed-events:
    cd finreport-rs && \
        {{local_env}} RUST_LOG=info \
        cargo run -p webapp --bin fixture-replay -- webapp/fixtures

# One command, clean checkout to a logged-in dashboard with seeded data:
# dev-up -> migrate -> seed-user -> seed-events -> projector (catch up) ->
# link every seeded account to `dev`. No bank credentials, no central broker.
dev-demo: dev-up
    just dev-migrate
    just seed-user
    just seed-events
    just dev-projector --until-caught-up
    cd finreport-rs && \
        {{local_env}} cargo run -p webapp --bin user-admin -- link --username dev --all
    @echo "==> Ready: just dev-be (backend) + just dev-fe (frontend), log in as dev/\${FINREPORT_PASSWORD:-dev}"

# Apply all migrations to the local `dev-up` Postgres (no .env needed).
dev-migrate:
    {{local_env}} sh -c 'sea-orm-cli migrate -d finreport-rs/migration -u "$APP_database_url" -s public'

# Wipe the projected read model (transactions, balances, offsets) so the next
# `dev-projector` run replays the log from the beginning. Deliberately leaves
# `account` (and every `user_account`/`app_user` row) in place: accounts are
# upserted back onto the same deterministic ids by the next replay (§2.3), so
# this never severs an existing account<->user link the way a `TRUNCATE …
# CASCADE` through the FK would.
dev-reset:
    docker compose -f docker-compose.local.yml exec -T finreport-be-postgres \
        psql -U finreport -d finreport \
        -c 'TRUNCATE TABLE transaction, account_balance, projection_offset;'

# Run the GraphQL backend locally against the Postgres started by `db-up`.
# Config comes from `local_env` (top of this file); no .env needed.
dev-be:
    cd finreport-rs && {{local_env}} RUST_LOG=info cargo run -p webapp --bin webapp

# Run the GraphQL backend locally against the tower (deployed) Postgres instead
# of the local one from `just db-up`. All other config still comes from
# finreport-rs/.env (see `dev-be`) — only APP_database_url is overridden here,
# with the real password pulled live from 1Password (never written to disk).
#
# WARNING: seaql::init_db() runs pending migrations on every startup. Running
# this applies any migration you've written locally — even ones not yet
# deployed — to the live tower database. Don't run this with unreviewed
# migrations sitting in finreport-rs/migration/.
dev-be-tower:
    cd finreport-rs && \
        APP_database_url="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.33:5432/finreport" \
        RUST_LOG=info \
        cargo run -p webapp --bin webapp

# Run the frontend locally, local profile (talks to `just dev-be` on localhost:8080).
dev-fe:
    cd finreport-fe && npm run dev

# Run the frontend locally, tower profile (talks to the deployed Unraid backend).
dev-fe-tower:
    cd finreport-fe && npm run dev:tower

# Run the importer locally against the Postgres started by `db-up`.
# Comdirect creds are pulled from 1Password via .env.tpl.
#
# Imports every account configured in .env.tpl, one task per login. Narrow it to
# a single login with `just import-local --account 1`.
import-local *ARGS:
    APP_database_url='postgresql://finreport:finreport@127.0.0.1:5432/finreport' \
        APP_oauth_url='https://api.comdirect.de' \
        APP_url='https://api.comdirect.de/api' \
        APP_save_file_path='.session.json' \
        RUST_LOG=info \
        op run --env-file .env.tpl -- \
        cargo run --manifest-path finreport-rs/Cargo.toml --bin import-transactions -- {{ARGS}}

# Runs the console container on this machine only — nothing is deployed.
# Open http://localhost:8090; Ctrl-C to stop.
#
# Kafka is central homelab infrastructure (kafka.lab.anydef.de), not part of
# this repo's stack, so this just needs kafka.lab.anydef.de to resolve and be
# reachable. Override either argument to point elsewhere, e.g. at a local
# broker:
#     just redpanda-console 127.0.0.1:19092
#     just redpanda-console kafka.lab.anydef.de:9092 9000
#
# Redpanda Console (web UI) locally, against the central broker.
redpanda-console broker="kafka.lab.anydef.de:9092" port="8090":
    @echo "==> Redpanda Console http://localhost:{{port}} -> {{broker}} (Ctrl-C to stop)"
    # Add -e REDPANDA_ADMINAPI_ENABLED=true -e REDPANDA_ADMINAPI_URLS=http://<broker-host>:9644
    # to surface cluster/broker config in the UI as well as topics.
    docker run --rm -it \
        --name finreport-redpanda-console \
        -p {{port}}:8080 \
        -e KAFKA_BROKERS={{broker}} \
        docker.redpanda.com/redpandadata/console:v2.8.5
