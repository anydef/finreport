locals {
  tag = "[tf]"
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

  extra_env = {
    POSTGRES_PASSWORD = var.postgres_password
  }
}