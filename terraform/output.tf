output "stack_id" {
  description = "ID of the deployed Portainer stack"
  value       = module.portainer_stack.stack_id
}

output "stack_name" {
  description = "Name of the deployed stack"
  value       = module.portainer_stack.stack_name
}

output "access_url" {
  description = "URL to access the deployed application"
  value       = "http://${var.app_host}:${var.app_port}"
}

output "portainer_stack_url" {
  description = "URL to view the stack in Portainer"
  value       = "${var.portainer_url}/#/stacks/${module.portainer_stack.stack_id}"
}

output "service_url" {
  description = "DNS-based URL for the service"
  value       = "https://finreport-be.lab.anydef.de"
}