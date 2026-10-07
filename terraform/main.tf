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

data "onepassword_vault" "homelab" {
  name = "HomeLab"
}

# Generated once and persisted in 1Password (below) — `admin`'s bootstrap
# (webapp/src/auth/bootstrap.rs) creates/rotates the `app_user` row from
# APP_admin_password at every startup, so changing this here is how you
# rotate the production admin password: `terraform apply` regenerates both
# the 1Password item and the stack's env, and the next `finreport-be` restart
# picks it up.
resource "random_password" "admin" {
  length = 32
  # Shell/URL-safe: no characters that need escaping in a shell command or a
  # query string, since this password also has to survive `docker run -e`
  # and the GraphQL login form without special handling.
  special = false
}

resource "onepassword_item" "finreport_admin" {
  vault    = data.onepassword_vault.homelab.uuid
  title    = "finreport admin"
  category = "login"
  username = "admin"
  password = random_password.admin.result
  url      = "https://finreport.lab.anydef.de"
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

resource "opnsense_haproxy_server" "finreport_fe" {
  name        = "FINREPORT_FE_server"
  description = "${local.tag} Server for finreport-fe at ${var.app_fe_host}:${var.app_fe_port}"
  address     = var.app_fe_host
  port        = tostring(var.app_fe_port)
}

resource "opnsense_haproxy_backend" "finreport_fe" {
  name           = "FINREPORT_FE_backend"
  description    = "${local.tag} Backend pool for finreport-fe (finreport.lab.anydef.de)"
  linked_servers = opnsense_haproxy_server.finreport_fe.id
}

resource "opnsense_haproxy_acl" "finreport_fe" {
  name        = "FINREPORT_FE_host_acl"
  description = "${local.tag} Match requests for finreport.lab.anydef.de"
  expression  = "hdr"
  value       = "finreport.lab.anydef.de"
}

resource "opnsense_haproxy_action" "finreport_fe" {
  name        = "FINREPORT_FE_rule"
  description = "${local.tag} Route finreport.lab.anydef.de to FINREPORT_FE_backend"
  type        = "use_backend"
  test_type   = "if"
  linked_acls = opnsense_haproxy_acl.finreport_fe.id
  operator    = "and"
  use_backend = opnsense_haproxy_backend.finreport_fe.id
}

resource "opnsense_haproxy_frontend_action" "finreport_fe" {
  frontend_id = data.opnsense_haproxy_frontend.https.id
  action_id   = opnsense_haproxy_action.finreport_fe.id
  prepend     = true
}

resource "opnsense_unbound_host_override" "finreport_fe" {
  # `finreport` (bare, no `-fe` suffix) is the user-facing name at
  # finreport.lab.anydef.de — finreport-be keeps its own `-be` host for the
  # API, distinct origins the CORS allow-list already depends on.
  hostname = "finreport"
  domain   = "lab.anydef.de"
  server   = "192.168.1.1"
}

resource "opnsense_haproxy_reconfigure" "apply" {
  depends_on = [
    opnsense_haproxy_frontend_action.finreport_be,
    opnsense_haproxy_frontend_action.finreport_fe,
  ]

  # depends_on only orders; without this an existing reconfigure is never
  # re-run, so newly added routes are saved in OPNsense but never applied
  # (HAProxy keeps the old config and answers 503 for the new host).
  lifecycle {
    replace_triggered_by = [
      opnsense_haproxy_server.finreport_be,
      opnsense_haproxy_backend.finreport_be,
      opnsense_haproxy_acl.finreport_be,
      opnsense_haproxy_action.finreport_be,
      opnsense_haproxy_frontend_action.finreport_be,
      opnsense_haproxy_server.finreport_fe,
      opnsense_haproxy_backend.finreport_fe,
      opnsense_haproxy_acl.finreport_fe,
      opnsense_haproxy_action.finreport_fe,
      opnsense_haproxy_frontend_action.finreport_fe,
    ]
  }
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
    {
      POSTGRES_PASSWORD     = var.postgres_password
      APP_anthropic_api_key = var.anthropic_api_key
      APP_admin_password    = random_password.admin.result
    },
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

# The broker is central homelab infrastructure (kafka.lab.anydef.de), managed
# and deployed outside this repo — finreport only owns its own topics on it.
# No readiness gate is needed here: an unreachable broker is a legitimate
# external-dependency failure for `terraform plan`/`apply` to report on its
# own, not something this module needs to poll for.
module "kafka_topics" {
  source = "./kafka"
}
