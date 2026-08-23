terraform {
  required_version = ">= 1.0"

  required_providers {
    portainer = {
      source  = "portainer/portainer"
      version = "~> 1.0"
    }
    restapi = {
      source  = "Mastercard/restapi"
      version = "~> 3.0"
    }
    opnsense = {
      source  = "registry.terraform.io/anydef/opnsense"
      version = "0.1.0"
    }
    kafka = {
      source  = "Mongey/kafka"
      version = "~> 0.13"
    }
    null = {
      source  = "hashicorp/null"
      version = "~> 3.0"
    }
  }

  backend "s3" {
    bucket = "terraform"
    key    = "finreport-be/terraform.tfstate"
    region = "garage"

    endpoints = {
      s3 = "http://192.168.1.234:9001"
    }

    skip_credentials_validation = true
    skip_metadata_api_check     = true
    skip_region_validation      = true
    skip_requesting_account_id  = true
    use_path_style              = true
    workspace_key_prefix        = "workspaces"
  }
}

provider "portainer" {
  endpoint = var.portainer_url
  api_key  = var.portainer_api_key
}

provider "restapi" {
  uri      = var.opnsense_url
  username = var.opnsense_api_key
  password = var.opnsense_api_secret
  insecure = true

  create_method         = "POST"
  update_method         = "POST"
  destroy_method        = "POST"
  read_method           = "GET"
  id_attribute          = "uuid"
  write_returns_object  = false
  create_returns_object = true
}

provider "opnsense" {
  url        = var.opnsense_url
  api_key    = var.opnsense_api_key
  api_secret = var.opnsense_api_secret
  insecure   = true
}

# Plaintext, no auth — the Redpanda broker sits on the trusted services LAN,
# not exposed beyond it, so TLS/SASL would add no real protection here.
#
# Note this couples plans of this module to broker reachability: once topics
# are in state, a refresh reads them. If the broker is down and you need to
# deploy anyway, `terraform apply -target=module.portainer_stack` (or
# `-refresh=false`) still works.
provider "kafka" {
  bootstrap_servers = var.kafka_bootstrap_servers
  tls_enabled       = false
}
