# ── Build stage ────────────────────────────────────────────────────────
ARG DOCKER_REGISTRY
FROM ${DOCKER_REGISTRY}/finreport-be-base:latest AS builder

WORKDIR /build

COPY finreport-rs/ ./

RUN cargo build --release --package webapp --bin webapp

# ── Runtime stage ─────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/webapp /usr/local/bin/finreport-be

# webapp reads `../assets/*` relative to its cwd — mirror the source layout
# so the relative paths resolve inside the container.
WORKDIR /app/webapp
COPY assets/  /app/assets/
COPY prompts/ /app/prompts/

EXPOSE 8080

ENTRYPOINT ["finreport-be"]