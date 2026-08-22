# finreport — Rust workspace + Docker (base + main image) + Portainer deploy
#
# Local dev override: BUILD_TOOLS_DIR=/path/to/build-tools just _bootstrap

set allow-duplicate-variables
set allow-duplicate-recipes

build_tools_dir   := ".build/build-tools"
docker_image_name := "finreport-be"

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

# Run Rust unit + integration tests across the workspace
test:
    cargo test --manifest-path finreport-rs/Cargo.toml

# Start local Postgres (via compose) in the background.
# No secrets needed here — POSTGRES_PASSWORD defaults in docker-compose.local.yml.
db-up:
    docker compose -f docker-compose.local.yml up finreport-be-postgres -d --wait

# Stop the local Postgres started by `db-up`.
db-down:
    docker compose -f docker-compose.local.yml down

# Run the GraphQL backend locally against the Postgres started by `db-up`.
# Config comes from finreport-rs/.env (copy finreport-rs/.env.example to create it) —
# cargo needs to run with CWD inside finreport-rs/ for dotenv() to find it.
dev-be:
    cd finreport-rs && RUST_LOG=info cargo run -p webapp --bin webapp

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