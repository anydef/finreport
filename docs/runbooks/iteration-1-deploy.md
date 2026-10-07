# Iteration 1 deploy runbook — Kafka-as-source-of-truth cutover

One-off runbook for cutting the deployed `finreport-be` stack over to the
iteration-1 shape (`docs/specs/iteration-1.md` §2.8/§9): importer publishes to
Kafka only, a new `finreport-be-projector` service builds the Postgres read
model, old tables survive renamed as `legacy_*`. Run every step from the repo
root unless noted. The user runs this manually — nothing here pushes or
deploys automatically.

Postgres is `192.168.100.46:5432` (`finreport-be-postgres`). Kafka is the
central broker `kafka.lab.anydef.de:9092`. The new projector container binds
`192.168.100.48` on `services-lan`.
Before deploying, confirm none of the stack's static IPs is in use:

```bash
docker network inspect services-lan \
  --format '{{range .Containers}}{{.Name}} {{.IPv4Address}}{{"\n"}}{{end}}' | sort -t. -k4 -n
```

If one is taken, `docker start` fails with "Address already in use" — move
that service's `ipv4_address` in `docker-compose.yml`.

---

## 1. Prerequisites

**Network.** `192.168.100.48` must reach both:

```bash
# From a host on services-lan (or exec into any container already on it):
nc -zv 192.168.100.46 5432
nc -zv kafka.lab.anydef.de 9092
```

`docker-compose.yml` puts `.36` on the same `services-lan` as `.32`
(`finreport-be`), `.33` (Postgres) and `.35` (importer), which already reach
both targets today — no Terraform resource manages a per-IP firewall rule for
this stack (`terraform/main.tf` only has `opnsense_haproxy_*` and
`opnsense_unbound_host_override` for `.32`; there is no `opnsense_firewall_*`
resource anywhere in `terraform/`). If OPNsense filters `services-lan` traffic
by individual IP rather than by subnet, add `.36` to the same allow rule as
`.32`/`.33`/`.35` by hand in the OPNsense UI before continuing — this repo's
Terraform does not express it, so it will not appear in a `terraform plan`.
`finreport-fe` (`.38`) only needs to reach `.32:8080` (GraphQL) — same caveat
applies if filtering is per-IP.

**New env vars / secrets.** No new secrets, and `.env.tpl`/
`terraform/variables.tf` need no new entries for this cutover —
`docker-compose.yml` now carries, as plain (non-secret) values:

- `finreport-be-projector` already sets `APP_database_url` and
  `APP_kafka_brokers`.
- `finreport-be` sets `APP_allowed_origins=https://finreport.lab.anydef.de`
  and `APP_cookie_secure=true`. The frontend (`finreport-fe`) is now deployed
  through this same compose file/`terraform/main.tf`, bound to
  `192.168.100.50` on `services-lan` and served over HTTPS at exactly this
  origin (OPNsense/HAProxy, same convention as `finreport-be.lab.anydef.de`),
  so both values are confirmed, not a guess — an empty/wrong allow-list means
  actix CORS rejects every request the SvelteKit proxy forwards (it relays
  the browser's own `Origin` header verbatim), and a frontend served over
  plain HTTP would need `APP_cookie_secure=false` instead, since a browser
  silently drops Secure cookies set over an insecure origin.
- `finreport-be` and `finreport-be-projector` both set
  `APP_run_migrations=false` — see §3 step 2; `just dev-be`/`dev-demo` are
  unaffected (the flag defaults to `true`, i.e. unchanged, when unset).
- `APP_projector_default_owner=<username>` on `finreport-be-projector` links
  every projected account to that user automatically instead of a manual
  `user-admin link --all` in §3 step 6. Not set today — leave the manual step
  if you don't want to touch `docker-compose.yml`.

**Verify the broker is reachable** before touching anything:

```bash
docker run --rm edenhill/kcat:1.7.1 -b kafka.lab.anydef.de:9092 -L | head -20
```

---

## 2. Backup

Dump the tower Postgres in custom format (needed for `pg_restore` in §5):

```bash
PGPASSWORD="$(op read 'op://HomeLab/finreport/psql/password')" \
  pg_dump -h 192.168.100.46 -p 5432 -U finreport -d finreport \
  -Fc -f finreport-pre-iter1-$(date +%Y%m%d%H%M%S).dump
```

Verify the dump is readable and lists the tables you expect
(`account`, `account_balance`, `account_transactions`) before relying on it:

```bash
pg_restore --list finreport-pre-iter1-*.dump | grep -E "TABLE (DATA )?public\.(account|account_balance|account_transactions)"
```

---

## 3. Push, build, cutover

**Push & CI build (user action).** Push `feat/iter1-mvp` and merge/push to
`main` — `.gitea/workflows/build-deploy.yaml` runs on push to `main`: it builds
and pushes the `finreport-be` image (`just build`) and then runs `just deploy`
(Terraform → Portainer), which reads `docker-compose.yml` with the new
`finreport-be-projector` block and the four static IPs. Wait for that workflow
to finish before continuing — the new image must exist in the registry before
the stack below can be rolled.

**Cutover order** (spec §2.8, exact steps):

1. **Stop the importer and webapp** — nothing may write while the schema moves:
   ```bash
   docker stop finreport-be-importer finreport-be
   ```
2. **Run migrations.** On a normal deploy this is automatic: the one-shot
   `finreport-be-migrate` compose service runs `up` and every DB-using service
   waits for it (`service_completed_successfully`). To run it by hand:
   Startup no longer runs `Migrator::up()`
   implicitly (`APP_run_migrations=false` on both `finreport-be` and
   `finreport-be-projector`, §1) — apply them with the `finreport-be-migrate`
   binary, now built into the same image (reads `DATABASE_URL`, **not**
   `APP_database_url`):
   ```bash
   docker run --rm --network services-lan \
     -e DATABASE_URL="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.46:5432/finreport" \
     --entrypoint finreport-be-migrate "${DOCKER_REGISTRY}/finreport-be:latest" up -s public
   ```
   This renames `account`/`account_balance`/`account_transactions` to
   `legacy_*` and creates the new `account`/`account_balance`/`transaction`/
   `app_user`/`user_account`/`user_session`/`projection_offset` tables.
3. **Run `legacy-backfill`** (read-only against Postgres, publishes to Kafka,
   key-skipping — safe to re-run), now built into the image instead of
   requiring a local checkout:
   ```bash
   docker run --rm --network services-lan \
     -e APP_database_url="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.46:5432/finreport" \
     -e APP_kafka_brokers="kafka.lab.anydef.de:9092" \
     --entrypoint legacy-backfill "${DOCKER_REGISTRY}/finreport-be:latest"
   ```
4. **Tombstone the watermark topic** (`finreport.import-watermark`) so the next
   import re-walks full history and republishes raw bank bytes, landing them
   *after* the backfill so compaction converges on raw data. The same
   `legacy-backfill` binary does this via `--tombstone-watermarks` — list the
   keys it would tombstone first, then publish for real:
   ```bash
   # Dry run — lists every live key, publishes nothing:
   docker run --rm --network services-lan \
     -e APP_kafka_brokers="kafka.lab.anydef.de:9092" \
     --entrypoint legacy-backfill "${DOCKER_REGISTRY}/finreport-be:latest" \
     --tombstone-watermarks --dry-run
   # Publish a null-value record for each key listed above:
   docker run --rm --network services-lan \
     -e APP_kafka_brokers="kafka.lab.anydef.de:9092" \
     --entrypoint legacy-backfill "${DOCKER_REGISTRY}/finreport-be:latest" \
     --tombstone-watermarks
   ```
5. **Start the projector, caught up first.** There's no standalone
   `--until-caught-up` invocation wired into `docker-compose.yml` (the deployed
   service always tails), so run it once to convergence before leaving it
   running long-term. Migrations already ran in step 2, so this throwaway run
   also passes `APP_run_migrations=false`:
   ```bash
   docker run --rm --network services-lan \
     -e APP_database_url="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.46:5432/finreport" \
     -e APP_kafka_brokers="kafka.lab.anydef.de:9092" \
     -e APP_run_migrations=false \
     --entrypoint projector "${DOCKER_REGISTRY}/finreport-be:latest" --until-caught-up
   # Then bring up the long-running, tailing instance:
   docker start finreport-be-projector
   ```
6. **Admin user**: `finreport-be`'s startup bootstrap
   (`webapp::auth::bootstrap`) now creates/maintains the `admin` user
   automatically, as soon as `finreport-be` starts in step 7 below — no
   manual step needed. Its password is generated by Terraform
   (`random_password.admin`) and stored in the 1Password item
   **"finreport admin"** (HomeLab vault); read it with
   `op read 'op://HomeLab/finreport admin/password'`. Bootstrap also links
   every projected account to `admin` on every startup (same effect as
   `APP_projector_default_owner=admin`, already set on
   `finreport-be-projector`, and as the manual `user-admin link --all` below).

   Fallback, if you need a second user or the bootstrap is disabled
   (`APP_admin_password` unset) — `user-admin`, same image, built into the
   runtime image:
   ```bash
   FINREPORT_PASSWORD='<choose one>' \
     APP_database_url="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.46:5432/finreport" \
     docker run --rm -i --network services-lan \
     -e APP_database_url -e FINREPORT_PASSWORD \
     --entrypoint user-admin "${DOCKER_REGISTRY}/finreport-be:latest" \
     create-user --username <you>
   # Same image/env, link every projected account (skip if
   # APP_projector_default_owner was set in §1):
   docker run --rm --network services-lan \
     -e APP_database_url --entrypoint user-admin \
     "${DOCKER_REGISTRY}/finreport-be:latest" link --username <you> --all
   ```
7. **Start the importer, webapp and frontend:**
   ```bash
   docker start finreport-be-importer finreport-be finreport-fe
   ```
   `finreport-fe` (`192.168.100.50:3000`, https://finreport.lab.anydef.de) is
   its own image built from `finreport-fe/Dockerfile` — redeploy it if its
   build changed; no backend restart is required for it specifically.

---

## 4. Verification checklist

```bash
# Row counts: legacy_* vs projected tables (allow for in-flight transactions
# published after the dump but before the stop in step 3.1).
PGPASSWORD="$(op read 'op://HomeLab/finreport/psql/password')" psql -h 192.168.100.46 -U finreport -d finreport -c "
  select 'legacy_account' , count(*) from legacy_account
  union all select 'account', count(*) from account
  union all select 'legacy_account_balance', count(*) from legacy_account_balance
  union all select 'account_balance', count(*) from account_balance
  union all select 'legacy_account_transactions', count(*) from legacy_account_transactions
  union all select 'transaction', count(*) from transaction;"

# projection_offset advancing (re-run after a few seconds; next_offset should grow
# while the importer's re-walk from step 3.4 is still catching up, then hold steady):
PGPASSWORD="$(op read 'op://HomeLab/finreport/psql/password')" psql -h 192.168.100.46 -U finreport -d finreport \
  -c "select topic, partition, next_offset, updated_at from projection_offset order by topic;"

# GraphQL login
curl -s -c cookies.txt -X POST http://192.168.100.45:8080/graphql \
  -H 'content-type: application/json' \
  -d '{"query":"mutation($u:String!,$p:String!){login(input:{username:$u,password:$p}){username}}","variables":{"u":"<you>","p":"<password>"}}'

# Dashboard renders — open the frontend, confirm the bar chart + Sankey +
# transaction list populate for the logged-in user.

# Importer publishes (tail its logs after step 3.7, expect Kafka publish lines,
# not Postgres writes):
docker logs -f --tail 50 finreport-be-importer
```

Clean up the local dump/cookie files once satisfied (`rm cookies.txt`).

---

## 5. Rollback

1. **Stop everything:**
   ```bash
   docker stop finreport-be-importer finreport-be finreport-be-projector
   ```
2. **Revert the schema with the new migrate binary**, while the image that
   still understands these migrations is the one running — do this *before*
   swapping back to the old image/binary, which doesn't know about them:
   ```bash
   docker run --rm --network services-lan \
     -e DATABASE_URL="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.46:5432/finreport" \
     --entrypoint finreport-be-migrate "${DOCKER_REGISTRY}/finreport-be:latest" down -n 2 -s public
   ```
   This reverses the two migrations this cutover introduced (new tables
   dropped, `legacy_*` renamed back to `account`/`account_balance`/
   `account_transactions`).
3. **Fallback: restore from the pre-cutover dump** instead, if `migrate down`
   fails or the result looks wrong (drops and recreates the schema — confirm
   you have the right file from §2):
   ```bash
   PGPASSWORD="$(op read 'op://HomeLab/finreport/psql/password')" \
     pg_restore -h 192.168.100.46 -U finreport -d finreport \
     --clean --if-exists -Fc finreport-pre-iter1-*.dump
   ```
4. **Redeploy the previous image tags.** Repoint `docker-compose.yml`'s
   `${DOCKER_REGISTRY}/finreport-be:latest` at the pre-cutover tag (or re-run
   `just deploy` against the prior commit) and drop the
   `finreport-be-projector` service/entrypoint from the stack definition —
   it has nothing to consume once Postgres is back on the old schema and the
   importer resumes writing it directly.
5. **Kafka topics are untouched by this rollback.** `finreport.account`,
   `.account-balance`, `.transaction` and `.import-watermark` all have
   `prevent_destroy` (`terraform/kafka/main.tf`) and nothing here deletes them.
   The backfilled/raw records already published stay on the topics; replaying
   them again later is safe (key-skipping in `legacy-backfill`, idempotent
   upserts in the projector) — rollback only reverts Postgres and the running
   images, not the event log.

---

## Gaps

Found while writing this runbook; none were invented around — flagged instead:

- **No per-IP OPNsense firewall resource in Terraform** for `services-lan`
  members (`.32`/`.33`/`.35`/`.36`/`.38`) — only `opnsense_haproxy_*` and
  `opnsense_unbound_host_override` exist for `.32`/`.38`. If filtering is
  per-IP rather than per-subnet, `.36`/`.38` need a manual OPNsense rule;
  nothing in `terraform plan` will show this as drift either way.
- **`docker-compose.yml` has no `finreport-be-projector --until-caught-up`
  one-shot step** — the deployed service always tails. §3 step 5 uses a
  throwaway `docker run` for the initial catch-up pass since the compose file
  doesn't express it.
- **`APP_projector_default_owner` is unset in `docker-compose.yml`** — every
  cutover currently needs the manual `user-admin link --all` in §3 step 6.
