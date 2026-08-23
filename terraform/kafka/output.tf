output "topic_names" {
  description = "Names of the managed finreport Kafka/Redpanda topics"
  value = [
    kafka_topic.account.name,
    kafka_topic.account_balance.name,
    kafka_topic.transaction.name,
    kafka_topic.import_watermark.name,
  ]
}
