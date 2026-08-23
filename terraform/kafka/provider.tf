# Child module of terraform/ — it declares which provider it needs, but the
# provider configuration and state live in the root module.
terraform {
  required_providers {
    kafka = {
      source  = "Mongey/kafka"
      version = "~> 0.13"
    }
  }
}
