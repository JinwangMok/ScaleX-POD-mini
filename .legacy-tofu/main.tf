terraform {
  required_providers {
    libvirt = {
      source  = "dmacvicar/libvirt"
      version = "~> 0.8"
    }
  }
}

provider "libvirt" {
  uri = "qemu+ssh://jinwang@100.64.0.1/system"
}

resource "libvirt_volume" "tower_base" {
  name   = "tower-base.qcow2"
  pool   = "default"
  source = "https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img"
  format = "qcow2"
}

resource "libvirt_volume" "tower" {
  name           = "tower-vm.qcow2"
  pool           = "default"
  base_volume_id = libvirt_volume.tower_base.id
  size           = var.disk_gb * 1024 * 1024 * 1024
  format         = "qcow2"
}

resource "libvirt_cloudinit_disk" "tower_init" {
  name      = "tower-init.iso"
  pool      = "default"
  user_data = templatefile("cloud-init/tower.cfg", {
    hostname    = "tower"
    k3s_version = var.k3s_version
    ssh_key     = file(var.ssh_public_key)
  })
}

resource "libvirt_domain" "tower" {
  name   = var.vm_name
  memory = var.memory_mb
  vcpu   = var.cpus

  disk {
    volume_id = libvirt_volume.tower.id
  }

  cloudinit = libvirt_cloudinit_disk.tower_init.id

  network_interface {
    bridge         = "br0"
    addresses      = [var.tower_ip]
    wait_for_lease = true
  }

  console {
    type        = "pty"
    target_type = "serial"
    target_port = "0"
  }
}
