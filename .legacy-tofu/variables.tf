variable "vm_name" {
  description = "Name of the tower VM"
  type        = string
  default     = "tower-vm"
}

variable "cpus" {
  description = "Number of vCPUs"
  type        = number
  default     = 2
}

variable "memory_mb" {
  description = "Memory in MB"
  type        = number
  default     = 2048
}

variable "disk_gb" {
  description = "Disk size in GB"
  type        = number
  default     = 20
}

variable "tower_ip" {
  description = "Static IP for the tower VM"
  type        = string
  default     = "192.168.88.100"
}

variable "k3s_version" {
  description = "k3s version to install"
  type        = string
  default     = "v1.33.1+k3s1"
}

variable "ssh_public_key" {
  description = "Path to SSH public key"
  type        = string
  default     = "~/.ssh/id_ed25519.pub"
}
