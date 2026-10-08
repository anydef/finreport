# Repair: LLM answers with a category slug that is not in the catalog

Before the fix, the labeler trusted whatever slug the LLM returned and hashed
it into a `category_id` (`category_uuid(slug)`) without checking the catalog.
When the model named a slug that is not in `category`:

- the label was written `status = resolved` with that dangling `category_id`;
- the projected `transaction_label` row ended up **resolved with a NULL
  category** (the slug lookup found nothing) - shown as "-" in the UI, left
  out of totals, and invisible to the unlabelled sweep because the row exists;
- the same dangling id was stored in `llm_label_cache`, so every later attempt
  was served the bad answer for free (`label_source = 'llm-cache'`).

The fix validates the slug (`processor::validate_suggestion`): an unknown slug
now becomes `needs_review` / `new_category` with the slug in
`proposed_category_path`, is cached as a proposal (no `category_id`), and
logs a `warn` ("provider returned a category slug that is not in the
catalog"). The labeler also ignores any existing cache entry whose
`category_id` is not in the catalog and re-asks the provider, so the cache
heals itself lazily. Rows already written still need the cleanup below. Run it
by hand against the deployed Postgres, in this order, in a transaction.

## 1. Inspect the damage (read-only)

```sql
-- Cache entries pointing at a category that does not exist.
SELECT c.fingerprint, c.category_id, c.confidence, c.provider, c.model, c.created_at
FROM llm_label_cache c
LEFT JOIN category cat ON cat.id = c.category_id
WHERE c.category_id IS NOT NULL
  AND cat.id IS NULL;

-- Labels that say "resolved" but have no category. Splits are legitimately
-- resolved with a NULL category (label_source = 'user'), so restrict to the
-- LLM sources.
SELECT l.transaction_id, l.label_source, l.confidence, l.fingerprint, l.reasoning
FROM transaction_label l
WHERE l.status = 'resolved'
  AND l.category_id IS NULL
  AND l.label_source IN ('llm', 'llm-cache');

-- Same, plus labels whose category_id points at a missing category row.
SELECT l.transaction_id, l.label_source, l.category_id
FROM transaction_label l
LEFT JOIN category cat ON cat.id = l.category_id
WHERE l.status = 'resolved'
  AND l.label_source IN ('llm', 'llm-cache')
  AND (l.category_id IS NULL OR cat.id IS NULL);
```

## 2. Repair

```sql
BEGIN;

-- Drop the poisoned cache entries so the next attempt asks the model again
-- (valid answers are re-cached; invalid ones become proposals).
DELETE FROM llm_label_cache c
WHERE c.category_id IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM category cat WHERE cat.id = c.category_id);

-- Drop the unusable labels. The transaction then has no label row, so the
-- unlabelled sweep picks it up and relabels it through the fixed path.
DELETE FROM transaction_label l
WHERE l.status = 'resolved'
  AND l.label_source IN ('llm', 'llm-cache')
  AND (l.category_id IS NULL
       OR NOT EXISTS (SELECT 1 FROM category cat WHERE cat.id = l.category_id));

COMMIT;
```

Then let the labeler's sweep run (or restart `finreport-be-labeler`). Budget:
each relabel is one LLM call, bounded per run by `APP_llm_max_requests_per_run`.

## Notes

- The Kafka topics `finreport.transaction-label` and `finreport.llm-cache`
  still hold the old bad records. That is harmless: the relabel publishes a
  newer record for the same key (compaction supersedes it), and the labeler
  ignores dangling cache entries even if a replay restores them.
- Relabelled transactions whose slug is still unknown will appear as
  `needs_review` / `new_category` with the model's slug in the proposed path.
  Check the warn log lines to see which slugs the model keeps inventing; a
  frequent one is a hint the taxonomy (`prompts/categories.json`) is missing
  a category.
