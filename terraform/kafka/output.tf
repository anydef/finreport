output "topic_names" {
  description = "Names of the managed finreport Kafka/Redpanda topics"
  value = [
    kafka_topic.account.name,
    kafka_topic.account_balance.name,
    kafka_topic.transaction.name,
    kafka_topic.import_watermark.name,
    kafka_topic.transaction_label.name,
    kafka_topic.llm_cache.name,
    kafka_topic.user_label.name,
    kafka_topic.rule.name,
    kafka_topic.category.name,
    kafka_topic.label_request.name,
  ]
}
