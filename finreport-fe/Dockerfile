# syntax=docker/dockerfile:1.4

# ── Build stage ────────────────────────────────────────────────────────
# node:22 matches the LTS line this project develops against; adapter-node's
# runtime has no native addons so the slim image is enough.
FROM node:22-bookworm-slim AS builder

WORKDIR /build

# Separate the lockfile copy from the rest of the source so dependency
# installs cache-hit across source-only changes.
COPY package.json package-lock.json ./
RUN --mount=type=cache,id=finreport-fe-npm,target=/root/.npm \
    npm ci

COPY . .
RUN npm run build && npm prune --omit=dev

# ── Runtime stage ─────────────────────────────────────────────────────
FROM node:22-bookworm-slim AS runtime

RUN groupadd --system --gid 1001 finreport-fe \
    && useradd --system --uid 1001 --gid finreport-fe finreport-fe

WORKDIR /app

COPY --from=builder --chown=finreport-fe:finreport-fe /build/build ./build
COPY --from=builder --chown=finreport-fe:finreport-fe /build/node_modules ./node_modules
COPY --from=builder --chown=finreport-fe:finreport-fe /build/package.json ./package.json

USER finreport-fe

ENV NODE_ENV=production
ENV PORT=3000
# Same-host default; the deployed stack overrides this to the real
# finreport.lab.anydef.de origin (docker-compose.yml).
ENV ORIGIN=http://localhost:3000

EXPOSE 3000

ENTRYPOINT ["node", "build"]
