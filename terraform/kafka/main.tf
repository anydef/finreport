# Kafka topic definitions for finreport, on the central homelab broker
# (kafka.lab.anydef.de — see the "kafka" provider block in terraform/provider.tf).
# The broker itself is infrastructure this repo does not own or deploy; this
# module owns only the finreport.* topics on it, as a child module of
# terraform/ sharing its state.
#
# All four topics are keyed by account_id.
#
# Deleting a topic deletes its events, and CI applies this module
# unattended on every push (see .gitea/workflows/build-deploy.yaml). So every
# topic here is guarded: an edit that Terraform would satisfy by REPLACING a
# topic — renaming it, lowering partitions — fails the apply instead of
# silently dropping the log. Ordinary in-place config changes (cleanup.policy,
# retention.ms) are unaffected. Removing a topic on purpose means deleting its
# lifecycle block first, deliberately, in a reviewed commit.

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

# Event stream: individual transactions per account_id. Raw Comdirect API
# JSON, byte-for-byte. Same reasoning as account-balance: every transaction
# event matters individually, so delete-cleanup with infinite retention
# rather than compaction.
resource "kafka_topic" "transaction" {
  name               = "finreport.transaction"
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
