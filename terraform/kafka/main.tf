# Kafka topic definitions for finreport, on the central homelab broker
# (kafka.lab.anydef.de — see the "kafka" provider block in terraform/provider.tf).
# The broker itself is infrastructure this repo does not own or deploy; this
# module owns only the finreport.* topics on it, as a child module of
# terraform/ sharing its state.
#
# account, account-balance and import-watermark are keyed by account_id;
# transaction is keyed by the transaction's own `reference` (see below).
#
# Deleting a topic deletes its events, and CI applies this module
# unattended on every push (see .gitea/workflows/build-deploy.yaml). So every
# topic here is guarded: an edit that Terraform would satisfy by REPLACING a
# topic — renaming it, lowering partitions — fails the apply instead of
# silently dropping the log. Ordinary in-place config changes (cleanup.policy,
# retention.ms) are unaffected. Removing a topic on purpose means deleting its
# lifecycle block first, deliberately, in a reviewed commit.
#
# docker-compose.local.yml's finreport-redpanda-init service recreates these
# same topics (name, partitions, cleanup.policy/retention.ms) against
# the local single-node Redpanda for `just dev-up`. It is not generated from
# this module — Terraform only ever touches the central broker — so a
# change here (new topic, different partitions/cleanup policy) needs the
# matching `rpk topic create` line there kept in step by hand.

# Entity snapshot: current state of a Comdirect account, one record per
# account_id. Raw Comdirect API JSON, byte-for-byte. Compacted (not
# time-deleted) because only the latest snapshot per key matters — old
# versions of the same account's state are useless once superseded.
resource "kafka_topic" "account" {
  name               = "finreport.account"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Event stream: balance-changed events per account_id. Raw Comdirect API
# JSON, byte-for-byte. Delete-cleanup (not compacted) because every event is
# meaningful on its own, not just the latest one, and retention is set to
# forever (-1) since this is the durable source of historical balances.
resource "kafka_topic" "account_balance" {
  name               = "finreport.account-balance"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "delete"
    "retention.ms"   = "-1"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Event stream: individual transactions, keyed by the transaction's own
# `reference` — not account_id. account_id is low-cardinality (one key per
# configured login), so it neither spreads load across partitions nor gives
# Kafka anything to dedupe on: a reimport (backfill, bug fix, new login)
# republishes every transaction in range as a brand new record, forever,
# since nothing here has ever depended on Kafka's per-partition ordering
# (the resume watermark is computed from the fetch batch in-process, not by
# replaying this topic — see webapp::kafka::watermark). Compacted on
# `reference` instead: a reimport just gives the log cleaner a newer record
# to keep for that key, so duplicates get GC'd instead of accumulating.
resource "kafka_topic" "transaction" {
  name               = "finreport.transaction"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Per-account import resume point (our own small JSON record, not raw
# Comdirect payload), keyed by account_id. Compacted because only the latest
# watermark per account is ever needed to resume an import — history of past
# watermarks has no value.
resource "kafka_topic" "import_watermark" {
  name               = "finreport.import-watermark"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# --- Iteration 2: labeling pipeline topics (docs/specs/iteration-2.md §2.2) ---
# These carry our own JSON, not a bank's raw payload, versioned by a
# schema_version field in both the payload and the iteration-1 envelope
# headers. Keyed by the transaction's own (source, external_id) identity
# (not the projected UUID), so they read with `rpk` without a Postgres
# lookup and survive a rebuild that hasn't run yet. Same partitions/RF/
# prevent_destroy posture as the ingest topics above, except label-request,
# which is a work queue, not state.

# Labeler output: one label per transaction, keyed `<source>:<external_id>`.
# Compacted — only the current label matters, never its history.
resource "kafka_topic" "transaction_label" {
  name               = "finreport.transaction-label"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# One LLM answer per fingerprint, independent of which transaction asked.
# Deliberately its own topic rather than derived from transaction-label (see
# §2.2): that topic compacts per transaction, so a rule or override replacing
# an LLM label would delete the only copy of the cached answer.
resource "kafka_topic" "llm_cache" {
  name               = "finreport.llm-cache"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Human overrides + splits, keyed `<source>:<external_id>`. Compacted: only
# the current override per transaction matters.
resource "kafka_topic" "user_label" {
  name               = "finreport.user-label"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Rule state (active, learned, revoked, ...), keyed by rule UUID. Compacted.
resource "kafka_topic" "rule" {
  name               = "finreport.rule"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Category tree nodes, keyed by category UUID. Compacted; categories are
# archived, never deleted, so a tombstone here is not expected in practice.
resource "kafka_topic" "category" {
  name               = "finreport.category"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Re-resolution work queue (`<source>:<external_id>` or `reapply:<rule-id>`),
# not state: a request is consumed once and has no lasting meaning after
# that, so unlike every topic above it is time-retained (7 days) rather than
# compacted, and NOT prevent_destroy — it is safe and expected to recreate
# this one if its shape ever needs to change.
resource "kafka_topic" "label_request" {
  name               = "finreport.label-request"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "delete"
    "retention.ms"   = "604800000"
  }
}

# Iteration 3 detector output (docs/specs/iteration-3.md §2.2): the auto
# layer for internal transfers and recurring costs, one row per
# transaction, keyed <source>:<external_id>. Compacted; a tombstone deletes
# the row (un-flagging).
resource "kafka_topic" "transaction_insight" {
  name               = "finreport.transaction-insight"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Iteration 4 goals (docs/specs/iteration-4.md §2.1): a user's spending or
# saving goal, keyed by the goal's own UUID, last-writer-wins like
# finreport.rule. Compacted; a tombstone deletes the goal.
resource "kafka_topic" "goal" {
  name               = "finreport.goal"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Learning exemptions: merchants (by normalised counterparty_key) the user has
# told the rule learner to leave alone. Compacted; a tombstone lifts the
# exemption. Mirrored in docker-compose.local.yml's finreport-redpanda-init.
resource "kafka_topic" "learning_exemption" {
  name               = "finreport.learning-exemption"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Display aliases: per-user nicknames for merchants (kind=counterparty, by
# normalised counterparty_key) and for the user's own connected accounts
# (kind=account, by account id). Keyed `<user_id>:<kind>:<key>`; compacted, a
# tombstone removes the alias. Mirrored in docker-compose.local.yml's
# finreport-redpanda-init.
resource "kafka_topic" "display_alias" {
  name = "finreport.display-alias"
  # Transaction links: a user-declared tie between transactions that offset one
  # another (a reimbursement and the expense it repays), keyed by the link's own
  # UUID. Compacted; a tombstone deletes the link. Mirrored in
  # docker-compose.local.yml's finreport-redpanda-init.
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }

}
resource "kafka_topic" "transaction_link" {
  name               = "finreport.transaction-link"
  partitions         = 1
  replication_factor = 1

  config = {
    "cleanup.policy" = "compact"
  }

  lifecycle {
    prevent_destroy = true
  }
}
