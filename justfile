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