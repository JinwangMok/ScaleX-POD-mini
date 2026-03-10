output "tower_ip" {
  description = "IP address of the tower VM"
  value       = var.tower_ip
}

output "tower_domain_name" {
  description = "Domain name of the tower VM"
  value       = libvirt_domain.tower.name
}
