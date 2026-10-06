# Iteration 1 deploy runbook — Kafka-as-source-of-truth cutover

One-off runbook for cutting the deployed `finreport-be` stack over to the
iteration-1 shape (`docs/specs/iteration-1.md` §2.8/§9): importer publishes to
Kafka only, a new `finreport-be-projector` service builds the Postgres read
model, old tables survive renamed as `legacy_*`. Run every step from the repo
root unless noted. The user runs this manually — nothing here pushes or
deploys automatically.

Postgres is `192.168.100.33:5432` (`finreport-be-postgres`). Kafka is the
central broker `kafka.lab.anydef.de:9092`. The new projector container binds
`192.168.100.36` on `services-lan`.

---

## 1. Prerequisites

**Network.** `192.168.100.36` must reach both:

```bash
# From a host on services-lan (or exec into any container already on it):
nc -zv 192.168.100.33 5432
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

**New env vars / secrets.** None are required to stand the projector up:
`docker-compose.yml`'s `finreport-be-projector` block already sets
`APP_database_url` and `APP_kafka_brokers`, and `.env.tpl`/`terraform/variables.tf`
need no new entries for this cutover. Optional:

- `APP_projector_default_owner=<username>` on `finreport-be-projector` links
  every projected account to that user automatically instead of a manual
  `user-admin link --all` in §3 step 7. Not set today — leave the manual step
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
  pg_dump -h 192.168.100.33 -p 5432 -U finreport -d finreport \
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
2. **Run migrations.** `webapp` normally runs `Migrator::up()` on every start
   (`db/seaql.rs::init_db`), but it's stopped, so apply them explicitly against
   the tower DB instead:
   ```bash
   cd finreport-rs
   DB_URL="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.33:5432/finreport" \
     sea-orm-cli migrate -u "$DB_URL" -s public
   ```
   This renames `account`/`account_balance`/`account_transactions` to
   `legacy_*` and creates the new `account`/`account_balance`/`transaction`/
   `app_user`/`user_account`/`user_session`/`projection_offset` tables.
3. **Run `legacy-backfill`** (read-only against Postgres, publishes to Kafka,
   key-skipping — safe to re-run):
   ```bash
   APP_database_url="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.33:5432/finreport" \
     APP_kafka_brokers="kafka.lab.anydef.de:9092" \
     cargo run --manifest-path finreport-rs/Cargo.toml --bin legacy-backfill
   ```
4. **Tombstone the watermark topic** (`finreport.import-watermark`) so the next
   import re-walks full history and republishes raw bank bytes, landing them
   *after* the backfill so compaction converges on raw data. No bin implements
   this (see Gaps) — do it with `kcat`, one tombstone per account key still
   live on the topic:
   ```bash
   # List the keys currently on the topic:
   docker run --rm edenhill/kcat:1.7.1 -b kafka.lab.anydef.de:9092 \
     -t finreport.import-watermark -C -e -K: -f 'key=%k\n' | sort -u
   # For each key printed above, publish a null-value record (a tombstone):
   echo "<account_id>:" | docker run --rm -i edenhill/kcat:1.7.1 \
     -b kafka.lab.anydef.de:9092 -t finreport.import-watermark -P -K:
   ```
5. **Start the projector, caught up first.** There's no standalone
   `--until-caught-up` invocation wired into `docker-compose.yml` (the deployed
   service always tails), so run it once to convergence before leaving it
   running long-term:
   ```bash
   docker run --rm --network finreport-be_services-lan \
     -e APP_database_url="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.33:5432/finreport" \
     -e APP_kafka_brokers="kafka.lab.anydef.de:9092" \
     --entrypoint projector "${DOCKER_REGISTRY}/finreport-be:latest" --until-caught-up
   # Then bring up the long-running, tailing instance:
   docker start finreport-be-projector
   ```
6. **Create the first user and link accounts** (`user-admin`, same image —
   see Gaps, this binary is not currently in the runtime image):
   ```bash
   FINREPORT_PASSWORD='<choose one>' \
     APP_database_url="postgresql://finreport:$(op read 'op://HomeLab/finreport/psql/password')@192.168.100.33:5432/finreport" \
     docker run --rm -i --network finreport-be_services-lan \
     -e APP_database_url -e FINREPORT_PASSWORD \
     --entrypoint user-admin "${DOCKER_REGISTRY}/finreport-be:latest" \
     create-user --username <you>
   # Same image/env, link every projected account (skip if
   # APP_projector_default_owner was set in §1):
   docker run --rm --network finreport-be_services-lan \
     -e APP_database_url --entrypoint user-admin \
     "${DOCKER_REGISTRY}/finreport-be:latest" link --username <you> --all
   ```
7. **Start the importer, webapp and frontend:**
   ```bash
   docker start finreport-be-importer finreport-be
   ```
   The frontend is static (`finreport-fe`, deployed separately per its own
   `CLAUDE.md`) — redeploy it if its build changed; no backend restart is
   required for it specifically.

---

## 4. Verification checklist

```bash
# Row counts: legacy_* vs projected tables (allow for in-flight transactions
# published after the dump but before the stop in step 3.1).
PGPASSWORD="$(op read 'op://HomeLab/finreport/psql/password')" psql -h 192.168.100.33 -U finreport -d finreport -c "
  select 'legacy_account' , count(*) from legacy_account
  union all select 'account', count(*) from account
  union all select 'legacy_account_balance', count(*) from legacy_account_balance
  union all select 'account_balance', count(*) from account_balance
  union all select 'legacy_account_transactions', count(*) from legacy_account_transactions
  union all select 'transaction', count(*) from transaction;"

# projection_offset advancing (re-run after a few seconds; next_offset should grow
# while the importer's re-walk from step 3.4 is still catching up, then hold steady):
PGPASSWORD="$(op read 'op://HomeLab/finreport/psql/password')" psql -h 192.168.100.33 -U finreport -d finreport \
  -c "select topic, partition, next_offset, updated_at from projection_offset order by topic;"

# GraphQL login
curl -s -c cookies.txt -X POST http://192.168.100.32:8080/graphql \
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
2. **Restore from the pre-cutover dump** (drops and recreates the schema —
   confirm you have the right file from §2):
   ```bash
   PGPASSWORD="$(op read 'op://HomeLab/finreport/psql/password')" \
     pg_restore -h 192.168.100.33 -U finreport -d finreport \
     --clean --if-exists -Fc finreport-pre-iter1-*.dump
   ```
3. **Redeploy the previous image tags.** Repoint `docker-compose.yml`'s
   `${DOCKER_REGISTRY}/finreport-be:latest` at the pre-cutover tag (or re-run
   `just deploy` against the prior commit) and drop the
   `finreport-be-projector` service/entrypoint from the stack definition —
   it has nothing to consume once Postgres is back on the old schema and the
   importer resumes writing it directly.
4. **Kafka topics are untouched by this rollback.** `finreport.account`,
   `.account-balance`, `.transaction` and `.import-watermark` all have
   `prevent_destroy` (`terraform/kafka/main.tf`) and nothing here deletes them.
   The backfilled/raw records already published stay on the topics; replaying
   them again later is safe (key-skipping in `legacy-backfill`, idempotent
   upserts in the projector) — rollback only reverts Postgres and the running
   images, not the event log.

---

## Gaps

Found while writing this runbook; none were invented around — flagged instead:

- **Dockerfile only builds/copies `webapp` and `import-transactions`**
  (`cargo build --release --package webapp --bin webapp --bin
  import-transactions`). `projector`, `user-admin` and `legacy-backfill` are
  separate `[[bin]]` targets in `finreport-rs/webapp/Cargo.toml` but are never
  built into the runtime image, even though `docker-compose.yml`'s
  `finreport-be-projector` service already sets `entrypoint: ["projector"]`
  against that same image. The Dockerfile needs those three bins added before
  §3 steps 3, 5 and 6 can run against the deployed image as written; until
  then they must run as `cargo run` from a checkout with network access to
  tower Postgres/Kafka (as shown above).
- **No bin/script tombstones the watermark topic.** §3 step 4 is spec'd
  (`docs/specs/iteration-1.md` §2.8) but unimplemented — the `kcat` commands
  above are a manual stand-in, not an existing documented procedure.
- **No per-IP OPNsense firewall resource in Terraform** for `services-lan`
  members (`.32`/`.33`/`.35`/`.36`) — only `opnsense_haproxy_*` and
  `opnsense_unbound_host_override` exist for `.32`. If filtering is per-IP
  rather than per-subnet, `.36` needs a manual OPNsense rule; nothing in
  `terraform plan` will show this as drift either way.
- **`docker-compose.yml` has no `finreport-be-projector --until-caught-up`
  one-shot step** — the deployed service always tails. §3 step 5 uses a
  throwaway `docker run` for the initial catch-up pass since the compose file
  doesn't express it.
- **`APP_projector_default_owner` is unset in `docker-compose.yml`** — every
  cutover currently needs the manual `user-admin link --all` in §3 step 6.
