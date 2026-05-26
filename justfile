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
db-up:
    op run --env-file .env.tpl -- \
        docker compose -f docker-compose.local.yml up finreport-be-postgres -d --wait

# Stop the local Postgres started by `db-up`.
db-down:
    docker compose -f docker-compose.local.yml down

# Run the importer locally against the Postgres started by `db-up`.
# Comdirect creds are pulled from 1Password via .env.tpl.
import-local:
    APP_database_url='postgresql://finreport:finreport@127.0.0.1:5432/finreport' \
        APP_oauth_url='https://api.comdirect.de' \
        APP_url='https://api.comdirect.de/api' \
        APP_save_file_path='.session.json' \
        RUST_LOG=info \
        op run --env-file .env.tpl -- \
        cargo run --manifest-path finreport-rs/Cargo.toml --bin import-transactions