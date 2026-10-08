# Repair: categories with a parent but a bare-leaf slug

A category's place in the tree is encoded in its slug (`personal.hobby`), and
every descendant match - category rollups, the spending breakdown, category
filters, goals scoped to a parent (`goals/scope.rs`,
`category_descendant_ids`) - is a dotted whole-segment prefix match on it.
`create_category` once accepted a child whose slug was **not** prefixed by its
parent's (`hobby` under `personal`), so those categories are invisible to all
of the above. (The mutation now validates the prefix; rows created before that
fix still need this repair.)

You cannot fix it with SQL alone: a category's id is `category_uuid(slug)`, so
a corrected slug is a *different id*, and the things that reference a category
(the user's own overrides on `finreport.user-label`, `finreport.rule`,
`finreport.category`) carry the **slug** on compacted topics. Repointing
Postgres would be silently undone by the next replay. `category-reslug`
republishes the events with the corrected slug and lets the projection follow.

## 1. Inspect (read-only SQL)

Broken categories (this is exactly what the tool discovers):

```sql
SELECT c.slug AS current_slug, p.slug AS parent,
       p.slug || '.' || split_part(c.slug, '.', array_length(string_to_array(c.slug, '.'), 1)) AS correct_slug,
       (SELECT count(*) FROM transaction_label  l WHERE l.category_id = c.id) AS labels,
       (SELECT count(*) FROM transaction_user_label u WHERE u.category_id = c.id) AS overrides,
       (SELECT count(*) FROM transaction_split  s WHERE s.category_id = c.id) AS splits,
       (SELECT count(*) FROM rule r WHERE r.category_id = c.id) AS rules
FROM category c
JOIN category p ON p.id = c.parent_id
WHERE left(c.slug, length(p.slug) + 1) <> p.slug || '.'
ORDER BY c.slug;
```

Zero rows means nothing is broken. After a real run the same query must return
zero rows, and none of the old slugs may remain:

```sql
SELECT count(*) FROM category c JOIN category p ON p.id = c.parent_id
WHERE left(c.slug, length(p.slug) + 1) <> p.slug || '.';          -- expect 0
```

Overrides must have survived (compare tags / recurring / note with a snapshot
taken before; `transaction_tag` is never touched by the tool):

```sql
SELECT u.transaction_id, cat.slug, u.recurring, u.note
FROM transaction_user_label u JOIN category cat ON cat.id = u.category_id
WHERE cat.slug IN ('personal.hobby', 'children.activities', 'children.childcare');
```

## 2. Dry run

```bash
docker run --rm --network services-lan \
  -e APP_database_url="postgresql://finreport:${POSTGRES_PASSWORD}@192.168.100.46:5432/finreport" \
  -e APP_kafka_brokers=kafka.lab.anydef.de:9092 \
  --entrypoint category-reslug "${DOCKER_REGISTRY}/finreport-be:latest" --dry-run
```

One line per broken category: `old`, `new`, `merged_into_existing`, and counts
of `labels` (transaction_label rows), `overrides` (user-label records on it),
`splits` (split parts on it) and `rules`; nothing is published or written.
A category the tool will not touch is logged as `SKIPPED` with the reason
(it has children, the corrected slug is too deep or already exists with a
different kind/parent, or its parent is itself broken).

## 3. Apply

Same command without `--dry-run`. `--only <current-slug>` (repeatable) limits a
run to named categories.

```
category-reslug [--dry-run] [--only <current-slug>]...
```

Order per category (the corrected category goes first, the old one is
tombstoned last, so nothing ever points at a missing category):

1. publish + project the corrected category (same name, kind, parent, origin;
   skipped - "merged" - if the corrected slug already exists with the same
   kind and parent);
2. republish + project every affected `user-label` record, whole-state with a
   fresh `revision`: tags, `recurring`, `note` and split parts are carried
   over and only the slugs (override and matching split parts) change;
3. republish + project every affected `rule` with the corrected slug
   (conditions, priority, state, origin, confidence, `user_touched` kept);
4. delete the stale `transaction_label` rows - the labeler's sweep re-resolves
   them and the precedence chain (override > rule > ...) puts them on the
   right category;
5. publish a tombstone for the old category and delete its row.

The tool reads the current `user-label` and `rule` records from Kafka (the
source of truth, not the possibly-lagging projection). If a record on those
topics cannot be parsed but mentions a broken slug, the run **aborts before
changing anything** rather than orphan it.

**Failure halfway is safe to re-run.** Every step is idempotent and the broken
row is removed last, so an interrupted run is still discovered next time; it
then reports `merged_into_existing=true` (the corrected category was already
created) and finishes the rest. A second full run finds nothing and reports
zero.

Not touched: `finreport.llm-cache` entries and `finreport.transaction-label`
records that still carry the old slug. The labeler ignores cache entries whose
slug is not in the catalog and re-asks, and label records are rewritten when
the sweep re-resolves.

## 4. Delivery

`docker-compose.yml` has a one-shot `finreport-be-category-reslug`
(`restart: "no"`, after `finreport-be-migrate`, like `repair-headers`). It ships
with `command: ["--dry-run"]`: a deploy only logs what it would do. Read the
container log, then change `command` to `[]` and redeploy to apply. Delete the
service once the validation fix has been live and the tool reports zero.
