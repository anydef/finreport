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

# Comdirect API credentials — consumed by the webapp/importer at runtime.
# Sourced from 1Password via TF_VAR_app_* env vars in .env.tpl.

variable "app_client_id" {
  description = "Comdirect API client_id"
  type        = string
  sensitive   = true
}

variable "app_client_secret" {
  description = "Comdirect API client_secret"
  type        = string
  sensitive   = true
}

variable "app_zugangsnummer" {
  description = "Comdirect online-banking access number"
  type        = string
  sensitive   = true
}

variable "app_pin" {
  description = "Comdirect online-banking PIN"
  type        = string
  sensitive   = true
}