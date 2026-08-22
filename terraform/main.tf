locals {
  tag = "[tf]"

  # Comdirect logins in importer order. An account whose client_id is blank is
  # not configured and is filtered out; index 0 must always be present.
  comdirect_accounts = [
    for account in [
      {
        client_id     = var.app_account_0_client_id
        client_secret = var.app_account_0_client_secret
        zugangsnummer = var.app_account_0_zugangsnummer
        pin           = var.app_account_0_pin
      },
      {
        client_id     = var.app_account_1_client_id
        client_secret = var.app_account_1_client_secret
        zugangsnummer = var.app_account_1_zugangsnummer
        pin           = var.app_account_1_pin
      },
    ] : account if account.client_id != ""
  ]
}

data "opnsense_haproxy_frontend" "https" {
  name = "1_HTTPS_frontend"
}

resource "opnsense_haproxy_server" "finreport_be" {
  name        = "FINREPORT_BE_server"
  description = "${local.tag} Server for finreport-be at ${var.app_host}:${var.app_port}"
  address     = var.app_host
  port        = tostring(var.app_port)
}

resource "opnsense_haproxy_backend" "finreport_be" {
  name           = "FINREPORT_BE_backend"
  description    = "${local.tag} Backend pool for finreport-be (finreport-be.lab.anydef.de)"
  linked_servers = opnsense_haproxy_server.finreport_be.id
}

resource "opnsense_haproxy_acl" "finreport_be" {
  name        = "FINREPORT_BE_host_acl"
  description = "${local.tag} Match requests for finreport-be.lab.anydef.de"
  expression  = "hdr"
  value       = "finreport-be.lab.anydef.de"
}

resource "opnsense_haproxy_action" "finreport_be" {
  name        = "FINREPORT_BE_rule"
  description = "${local.tag} Route finreport-be.lab.anydef.de to FINREPORT_BE_backend"
  type        = "use_backend"
  test_type   = "if"
  linked_acls = opnsense_haproxy_acl.finreport_be.id
  operator    = "and"
  use_backend = opnsense_haproxy_backend.finreport_be.id
}

resource "opnsense_haproxy_frontend_action" "finreport_be" {
  frontend_id = data.opnsense_haproxy_frontend.https.id
  action_id   = opnsense_haproxy_action.finreport_be.id
  prepend     = true
}

resource "opnsense_unbound_host_override" "finreport_be" {
  hostname = "finreport-be"
  domain   = "lab.anydef.de"
  server   = "192.168.1.1"
}

resource "opnsense_haproxy_reconfigure" "apply" {
  depends_on = [
    opnsense_haproxy_frontend_action.finreport_be,
  ]
}

module "portainer_stack" {
  source = "github.com/anydef/build-tools//terraform/portainer-stack?ref=main"

  stack_name         = var.stack_name
  endpoint_id        = var.endpoint_id
  stack_file_content = file("${path.module}/../docker-compose.yml")
  docker_registry    = var.docker_registry
  force_update       = var.force_update

  # Each configured login is flattened into the numbered APP_accounts__<n>__*
  # form utils::settings reads. Accounts left empty are dropped, so the stack
  # never receives half-populated credentials.
  extra_env = merge(
    { POSTGRES_PASSWORD = var.postgres_password },
    merge([
      for index, account in local.comdirect_accounts : {
        "APP_accounts__${index}__client_id"     = account.client_id
        "APP_accounts__${index}__client_secret" = account.client_secret
        "APP_accounts__${index}__zugangsnummer" = account.zugangsnummer
        "APP_accounts__${index}__pin"           = account.pin
      }
    ]...)
  )
}