# finreport — Rust workspace + Docker (base + main image) + Portainer deploy
#
# Local dev override: BUILD_TOOLS_DIR=/path/to/build-tools just _bootstrap

set allow-duplicate-variables
set allow-duplicate-recipes

build_tools_dir   := ".build/build-tools"
docker_image_name := "finreport-be"

# Shared cargo target dir across worktrees (docs/specs/iteration-2.md §9.1):
# every worktree builds into the same directory, sibling to
# `finreport-worktrees/`, so N parallel agents share one dependency build
# instead of each paying a cold compile. Cargo locks the directory, so
# concurrent builds serialize rather than corrupt each other. Computed, not
# hardcoded, so it resolves correctly whether `just` runs from a worktree
# (.../finreport-worktrees/<branch>/) or the main checkout (.../finreport/,
# sibling to finreport-worktrees/) — see "Multi-agent development" in
# CLAUDE.md for the directory layout this assumes. A `CARGO_TARGET_DIR`
# already set in the calling shell overrides this.
export CARGO_TARGET_DIR := env_var_or_default("CARGO_TARGET_DIR", `
    # Resolved from git, not from the path: every worktree of this repo shares
    # one git-common-dir, which always points at the MAIN checkout regardless
    # of where just runs from. Deriving it from pwd instead assumed every
    # worktree is a sibling under finreport-worktrees/, and silently started a
    # second full target dir for any that is not. Agent worktrees live under
    # .claude/worktrees/agent-*/, so that assumption produced a 13G duplicate
    # of a cache that already existed, and a cold compile each time.
    common="$(git rev-parse --git-common-dir 2>/dev/null || true)"
    if [ -n "$common" ] && [ -d "$common" ]; then
        main_checkout="$(dirname "$(cd "$common" && pwd)")"
        echo "$(dirname "$main_checkout")/finreport-worktrees/.cargo-target"
    else
        # Not a git checkout (an extracted tarball, say): keep the old
        # sibling-layout guess rather than failing outright.
        parent="$(dirname "$(pwd)")"
        if [ "$(basename "$parent")" = "finreport-worktrees" ]; then
            echo "$parent/.cargo-target"
        else
            echo "$parent/finreport-worktrees/.cargo-target"
        fi
    fi
`)

# Local-dev config for every recipe that talks to the `dev-up` stack, so no
# finreport-rs/.env (or 1Password) is needed to run the demo. Each value can
# still be overridden from the calling shell. These are throwaway localhost
# defaults, never real secrets; real credentials only come in via .env.tpl.
local_env := 'APP_database_url="${APP_database_url:-postgresql://finreport:${POSTGRES_PASSWORD:-finreport}@127.0.0.1:${FINREPORT_PG_PORT:-5432}/finreport}" APP_kafka_brokers="${APP_kafka_brokers:-127.0.0.1:${FINREPORT_KAFKA_PORT:-19092}}" APP_cookie_secure="${APP_cookie_secure:-false}" APP_allowed_origins="${APP_allowed_origins:-http://localhost:5173}"'

import? '.build/build-tools/common.just'

# Build and push the frontend image, same script as the backend's `build`
# recipe (from common.just) but pointed at finreport-fe/Dockerfile instead —
# a second recipe rather than reusing `build` because that recipe's
# `docker_image_name`/`build_context` are fixed to the backend image at
# import time.
build-fe:
    DOCKER_IMAGE_NAME=finreport-fe \
    BUILD_CONTEXT={{justfile_directory()}}/finreport-fe \
    {{build_tools_dir}}/build-and-push.sh

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

# Run the labeler locally (docs/specs/iteration-2.md §2.3) against the stack
# from `dev-up`: consumes finreport.transaction/.user-label/.rule/
# .label-request and resolves labels into the local Postgres. Pass
# `--until-caught-up` to exit at the log end (what `dev-demo` uses); omit it
# to keep tailing like the deployed service. Refuses to start while the
# projector lags behind the broker by more than APP_labeler_max_projection_lag
# records (default 0) — run `dev-projector --until-caught-up` first.
#
# Config comes from `local_env` (top of this file); no .env needed, no API
# key needed (APP_llm_provider defaults to the zero-cost `fake` provider).
dev-labeler *ARGS:
    cd finreport-rs && \
        {{local_env}} RUST_LOG=info \
        cargo run -p webapp --bin labeler -- {{ARGS}}

# Idempotently publish the iteration-2 taxonomy (prompts/taxonomy.json) onto
# finreport.category, so the labeler has a catalog to resolve against. Safe
# to re-run: re-publishing an unchanged category is a no-op at the projector.
seed-categories:
    cd finreport-rs && \
        {{local_env}} RUST_LOG=info \
        cargo run -p webapp --bin category-seed -- ../prompts/taxonomy.json

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

# One command, clean checkout to a logged-in dashboard with a populated
# review queue and at least one learned rule (docs/specs/iteration-2.md §7):
#
#   dev-up -> migrate -> seed-user -> seed-events -> seed-categories
#   -> repeat { projector --until-caught-up ; labeler --until-caught-up }
#      until a round produces no new records (max 5 rounds, else fail loudly)
#   -> link accounts -> assert -> print next steps
#
# A single projector -> seed-categories -> labeler pass cannot work: the
# seeded categories are only *published* at that step, so the labeler would
# start against an empty catalog, and the labels it publishes would never be
# projected, so the learner (which reads the projection) would see nothing.
# Each round is cheap — both binaries exit at the high watermark — and it
# converges: round 1 projects categories and transactions, round 2 labels
# them and projects the labels, round 3 lets the learner see enough history
# to emit the learned rule, round 4 projects it.
#
# No bank credentials, no central broker, no API key — the fake LLM provider
# is the default (APP_llm_provider).
dev-demo: dev-up
    just dev-migrate
    just seed-user
    just seed-events
    just seed-categories
    # Accounts must be projected and linked to the demo user *before* the
    # detection pass runs (iteration-3 §3.1 rule 2: a transfer pair needs a
    # common owning user) — otherwise every `_dev-demo-loop` round detects
    # zero transfers no matter how many times it repeats.
    just dev-projector --until-caught-up
    cd finreport-rs && \
        {{local_env}} cargo run -p webapp --bin user-admin -- link --username dev --all
    just _dev-demo-loop
    just _dev-demo-assert
    @echo "==> Ready: just dev-be (backend) + just dev-fe (frontend), log in as dev/\${FINREPORT_PASSWORD:-dev}"

# The alternating projector/labeler loop (§7), split out so `dev-demo`'s own
# body can stay a plain sequence of `just` calls (a shebang script has to be
# the whole recipe body, not just part of it).
[private]
_dev-demo-loop:
    #!/usr/bin/env bash
    set -euo pipefail
    cd finreport-rs
    round=0
    while :; do
        round=$((round + 1))
        if [ "$round" -gt 5 ]; then
            echo "==> dev-demo: 5 rounds of projector/labeler and still not quiescent — failing loudly" >&2
            exit 1
        fi
        before=$(docker compose -f ../docker-compose.local.yml exec -T finreport-be-postgres \
            psql -U finreport -d finreport -tAc \
            'SELECT (SELECT count(*) FROM transaction) + (SELECT count(*) FROM transaction_label) + (SELECT count(*) FROM rule);')
        {{local_env}} RUST_LOG=info cargo run -p webapp --bin projector -- --until-caught-up
        # The WP0 demo fixtures (webapp/fixtures) use fixed 2024 dates for the
        # recurring-series cases; a wide window keeps the demo's recurring
        # detection independent of wall-clock "today" instead of letting it
        # silently stop asserting anything once 2024 falls outside the
        # default 18-month lookback.
        {{local_env}} APP_recurring_window_months="${APP_recurring_window_months:-9999}" \
            RUST_LOG=info cargo run -p webapp --bin labeler -- --until-caught-up
        after=$(docker compose -f ../docker-compose.local.yml exec -T finreport-be-postgres \
            psql -U finreport -d finreport -tAc \
            'SELECT (SELECT count(*) FROM transaction) + (SELECT count(*) FROM transaction_label) + (SELECT count(*) FROM rule);')
        echo "==> dev-demo: round $round (transaction+transaction_label+rule rows: $before -> $after)"
        if [ "$before" = "$after" ]; then
            break
        fi
    done

# A silently empty demo is worse than a broken one (§7): fail loudly unless
# at least one learned rule and at least one held-for-review label exist.
[private]
_dev-demo-assert:
    #!/usr/bin/env bash
    set -euo pipefail
    learned=$(docker compose -f docker-compose.local.yml exec -T finreport-be-postgres \
        psql -U finreport -d finreport -tAc \
        "SELECT count(*) FROM rule WHERE origin = 'learned';")
    review=$(docker compose -f docker-compose.local.yml exec -T finreport-be-postgres \
        psql -U finreport -d finreport -tAc \
        "SELECT count(*) FROM transaction_label WHERE status = 'needs_review';")
    if [ "${learned:-0}" -lt 1 ] || [ "${review:-0}" -lt 1 ]; then
        echo "==> dev-demo: assertion failed — learned rules: ${learned:-0}, needs_review labels: ${review:-0}" >&2
        exit 1
    fi
    echo "==> dev-demo: asserted ${learned} learned rule(s), ${review} needs_review label(s)"
    transfers=$(docker compose -f docker-compose.local.yml exec -T finreport-be-postgres \
        psql -U finreport -d finreport -tAc \
        "SELECT count(*) FROM transaction_insight WHERE is_transfer;")
    recurring_series=$(docker compose -f docker-compose.local.yml exec -T finreport-be-postgres \
        psql -U finreport -d finreport -tAc \
        "SELECT count(*) FROM (SELECT recurring_series_id FROM transaction_insight WHERE is_recurring GROUP BY recurring_series_id) s;")
    if [ "${transfers:-0}" -lt 1 ] || [ "${recurring_series:-0}" -lt 1 ]; then
        echo "==> dev-demo: assertion failed — transfer legs: ${transfers:-0}, recurring series: ${recurring_series:-0}" >&2
        exit 1
    fi
    echo "==> dev-demo: asserted ${transfers} transfer leg(s) across >=1 pair, ${recurring_series} recurring series"

# Apply all migrations to the local `dev-up` Postgres (no .env needed).
dev-migrate:
    {{local_env}} sh -c 'sea-orm-cli migrate -d finreport-rs/migration -u "$APP_database_url" -s public'

# Wipe the projected read model (transactions, balances, offsets, labels,
# rules, categories) so the next projector/labeler run replays the log from
# the beginning. Deliberately leaves
# `account` (and every `user_account`/`app_user` row) in place: accounts are
# upserted back onto the same deterministic ids by the next replay (§2.3), so
# this never severs an existing account<->user link the way a `TRUNCATE …
# CASCADE` through the FK would.
dev-reset:
    docker compose -f docker-compose.local.yml exec -T finreport-be-postgres \
        psql -U finreport -d finreport \
        -c 'TRUNCATE TABLE transaction, account_balance, projection_offset, transaction_label, transaction_user_label, transaction_split, llm_label_cache, rule, category;'

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
        APP_database_url="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.46:5432/finreport" \
        RUST_LOG=info \
        cargo run -p webapp --bin webapp

# Run the frontend locally, local profile (talks to `just dev-be` on localhost:8080).
#
# Reinstalls first if node_modules is missing or older than package-lock.json
# (e.g. after a merge bumps a dependency) — a stale node_modules otherwise
# fails at runtime with something like "Cannot find module 'layerchart'"
# instead of at install time.
dev-fe:
    cd finreport-fe && \
        ( [ node_modules/.package-lock.json -nt package-lock.json ] 2>/dev/null || npm ci ) && \
        npm run dev

# Run the frontend locally, tower profile (talks to the deployed Unraid backend).
# Every GraphQL operation is served from the fixtures in
# `finreport-fe/src/lib/graphql/mocks/` via `PUBLIC_USE_MOCKS=1`: no Postgres,
# no Kafka, no `dev-be`. This is the path a UI change is reviewed through
# before any backend work starts (see "UI work starts with mocks" in CLAUDE.md).
# Run the frontend on mocked data only — no backend of any kind.
dev-fe-mocks:
    cd finreport-fe && \
        ( [ node_modules/.package-lock.json -nt package-lock.json ] 2>/dev/null || npm ci ) && \
        npm run dev:mocks

dev-fe-tower:
    cd finreport-fe && \
        ( [ node_modules/.package-lock.json -nt package-lock.json ] 2>/dev/null || npm ci ) && \
        npm run dev:tower

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
