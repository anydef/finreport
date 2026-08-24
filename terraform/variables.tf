variable "portainer_url" {
  description = "Portainer instance URL"
  type        = string
  default     = "http://192.168.1.234:9000"
}

variable "portainer_api_key" {
  description = "Portainer API access token"
  type        = string
  sensitive   = true
  # Set via environment variable TF_VAR_portainer_api_key
}

variable "docker_registry" {
  description = "Docker registry address"
  type        = string
  default     = "registry.lab.anydef.de"
}

variable "stack_name" {
  description = "Name of the Portainer stack"
  type        = string
  default     = "finreport-be"
}

variable "endpoint_id" {
  description = "Portainer endpoint ID (check Portainer UI or API for correct ID)"
  type        = number
  default     = 3
}

variable "force_update" {
  description = "Set to a new value (e.g., timestamp) to force stack recreation"
  type        = string
  default     = ""
}

variable "opnsense_url" {
  description = "OPNsense base URL"
  type        = string
  default     = "https://192.168.1.1"
}

variable "opnsense_api_key" {
  description = "OPNsense API key"
  type        = string
  sensitive   = true
}

variable "opnsense_api_secret" {
  description = "OPNsense API secret"
  type        = string
  sensitive   = true
}

variable "app_host" {
  description = "Host/IP where the application is reachable on services-lan"
  type        = string
  default     = "192.168.100.32"
}

variable "app_port" {
  description = "Port the application listens on"
  type        = number
  default     = 8080
}

variable "postgres_password" {
  description = "Postgres password for the finreport role"
  type        = string
  sensitive   = true
  # Set via TF_VAR_postgres_password, sourced from op://HomeLab/finreport/psql/password in .env.tpl
}

# Comdirect API credentials — consumed by the importer at runtime, one numbered
# block per online-banking login. Sourced from 1Password via the
# TF_VAR_app_account_<n>_* entries in .env.tpl.
#
# Deliberately plain strings rather than a list(object): a complex variable has
# to arrive as one JSON-encoded TF_VAR, and .env.tpl cannot compose one — CI
# loads that file with 1password/load-secrets-action, which resolves one op://
# reference per line and performs no shell expansion.
#
# Account 0 has no default on purpose: a missing secret must fail the deploy
# rather than quietly ship a container with blank credentials.

variable "app_account_0_client_id" {
  description = "Comdirect API client_id for account 0"
  type        = string
  sensitive   = true
}

variable "app_account_0_client_secret" {
  description = "Comdirect API client_secret for account 0"
  type        = string
  sensitive   = true
}

variable "app_account_0_zugangsnummer" {
  description = "Comdirect online-banking access number for account 0"
  type        = string
  sensitive   = true
}

variable "app_account_0_pin" {
  description = "Comdirect online-banking PIN for account 0"
  type        = string
  sensitive   = true
}

# Account 1 — optional. Left empty it is skipped entirely, so no blank
# APP_accounts__1__* variables reach the stack. Copy this block for a third.

variable "app_account_1_client_id" {
  description = "Comdirect API client_id for account 1 (optional)"
  type        = string
  sensitive   = true
  default     = ""
}

variable "app_account_1_client_secret" {
  description = "Comdirect API client_secret for account 1 (optional)"
  type        = string
  sensitive   = true
  default     = ""
}

variable "app_account_1_zugangsnummer" {
  description = "Comdirect online-banking access number for account 1 (optional)"
  type        = string
  sensitive   = true
  default     = ""
}

variable "app_account_1_pin" {
  description = "Comdirect online-banking PIN for account 1 (optional)"
  type        = string
  sensitive   = true
  default     = ""
}

variable "kafka_bootstrap_servers" {
  description = "Central homelab Kafka bootstrap servers (not deployed by this repo)"
  type        = list(string)
  default     = ["kafka.lab.anydef.de:9092"]
}
