use crate::models::sdi::{NodeSpec, SdiSpec};

/// Input for host-level infrastructure generation.
/// Represents a bare-metal host that should be set up as a libvirt hypervisor.
#[derive(Clone, Debug)]
pub struct HostInfraInput {
    pub name: String,
    pub ip: String,
    pub ssh_user: String,
}

/// Generate OpenTofu HCL for host-level libvirt infrastructure.
/// Sets up libvirt providers, storage pools, and outputs for each bare-metal host.
/// Pure function: takes host list + network config, returns HCL string.
pub fn generate_tofu_host_infra(
    hosts: &[HostInfraInput],
    _bridge: &str,
    _mgmt_cidr: &str,
    _gateway: &str,
) -> String {
    let mut hcl = String::new();

    // Terraform block
    hcl.push_str(&generate_terraform_block());
    hcl.push('\n');

    // Provider blocks — one per host (SSH-based libvirt)
    for host in hosts {
        hcl.push_str(&format!(
            r#"provider "libvirt" {{
  alias = "{name}"
  uri   = "qemu+ssh://{ssh_user}@{ip}/system?no_verify=1"
}}
"#,
            name = host.name,
            ssh_user = host.ssh_user,
            ip = host.ip,
        ));
        hcl.push('\n');
    }

    // Storage pool on each host
    for host in hosts {
        hcl.push_str(&format!(
            r#"resource "libvirt_pool" "scalex_pool_{name}" {{
  name = "scalex-pool"
  type = "dir"
  path = "/var/lib/libvirt/scalex-pool"
  provider = libvirt.{name}
}}
"#,
            name = host.name,
        ));
        hcl.push('\n');
    }

    // Outputs — host pool status
    hcl.push_str("# --- Host Infrastructure Outputs ---\n");
    for host in hosts {
        hcl.push_str(&format!(
            r#"output "{name}_pool_id" {{
  value = libvirt_pool.scalex_pool_{name}.id
}}
"#,
            name = host.name,
        ));
    }

    hcl
}

/// Generate OpenTofu HCL for a complete SDI spec.
/// Pure function: returns HCL string without writing files.
pub fn generate_tofu_main(spec: &SdiSpec, ssh_user: &str) -> String {
    let mut hcl = String::new();

    // Terraform block
    hcl.push_str(&generate_terraform_block());
    hcl.push('\n');

    // Provider blocks — one per unique host
    let hosts = collect_unique_hosts(spec);
    for host in &hosts {
        hcl.push_str(&generate_provider_block(host, ssh_user));
        hcl.push('\n');
    }

    // Base image volume per host
    for host in &hosts {
        hcl.push_str(&generate_base_volume(
            host,
            &spec.os_image.source,
            &spec.os_image.format,
        ));
        hcl.push('\n');
    }

    // Cloud-init common data
    hcl.push_str(&generate_cloudinit_data(spec));
    hcl.push('\n');

    // VM resources for each pool
    for pool in &spec.spec.sdi_pools {
        hcl.push_str(&format!(
            "# --- Pool: {} ({}) ---\n",
            pool.pool_name, pool.purpose
        ));
        for node in &pool.node_specs {
            let host = resolve_node_host(node, &pool.placement.hosts);
            hcl.push_str(&generate_vm_resource(
                node,
                &host,
                &spec.resource_pool.network.management_bridge,
                &spec.resource_pool.network.gateway,
            ));
            hcl.push('\n');
        }
    }

    // Outputs
    hcl.push_str(&generate_outputs(spec));

    hcl
}

/// Collect all unique host names from the spec
pub fn collect_unique_hosts(spec: &SdiSpec) -> Vec<String> {
    let mut hosts = Vec::new();
    for pool in &spec.spec.sdi_pools {
        for node in &pool.node_specs {
            let host = resolve_node_host(node, &pool.placement.hosts);
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
    }
    hosts
}

/// Resolve which physical host a VM should run on
fn resolve_node_host(node: &NodeSpec, pool_hosts: &[String]) -> String {
    if let Some(ref host) = node.host {
        host.clone()
    } else if let Some(first) = pool_hosts.first() {
        first.clone()
    } else {
        "localhost".to_string()
    }
}

fn generate_terraform_block() -> String {
    r#"terraform {
  required_providers {
    libvirt = {
      source  = "dmacvicar/libvirt"
      version = "~> 0.7.0"
    }
  }
}
"#
    .to_string()
}

fn generate_provider_block(host: &str, ssh_user: &str) -> String {
    // For localhost, use local qemu connection; otherwise SSH
    if host == "localhost" {
        r#"provider "libvirt" {
  uri = "qemu:///system"
}
"#
        .to_string()
    } else {
        format!(
            r#"provider "libvirt" {{
  alias = "{host}"
  uri   = "qemu+ssh://{ssh_user}@{host}/system?no_verify=1"
}}
"#
        )
    }
}

fn generate_base_volume(host: &str, source: &str, format: &str) -> String {
    let alias = if host == "localhost" {
        String::new()
    } else {
        format!("\n  provider = libvirt.{host}")
    };
    format!(
        r#"# Base volume is pre-created via virsh (SSH upload avoids provider timeout)
# Referenced by name in disk volumes via base_volume_name

"#
    )
}

fn generate_cloudinit_data(spec: &SdiSpec) -> String {
    let packages = spec
        .cloud_init
        .packages
        .iter()
        .map(|p| format!("    - {p}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"data "template_file" "cloud_init" {{
  template = <<-EOF
#cloud-config
ssh_authorized_keys:
  - ${{file("{ssh_key}")}}
packages:
{packages}
package_update: true
package_upgrade: true
runcmd:
  - systemctl enable --now iscsid
  - modprobe br_netfilter
  - sysctl -w net.ipv4.ip_forward=1
  - sysctl -w net.bridge.bridge-nf-call-iptables=1
EOF
}}
"#,
        ssh_key = spec.cloud_init.ssh_authorized_keys_file,
    )
}

fn generate_vm_resource(node: &NodeSpec, host: &str, bridge: &str, gateway: &str) -> String {
    let provider = if host == "localhost" {
        String::new()
    } else {
        format!("\n  provider = libvirt.{host}")
    };

    let gpu_passthrough = node.devices.as_ref().is_some_and(|d| d.gpu_passthrough);

    let xml_block = if gpu_passthrough {
        r#"
  xml {
    xslt = file("${path.module}/vfio-passthrough.xslt")
  }"#
        .to_string()
    } else {
        String::new()
    };

    let name = &node.node_name;
    let mem = node.mem_gb as u64 * 1024; // MiB
    let vcpu = node.cpu;
    let disk_gb = node.disk_gb as u64 * 1024 * 1024 * 1024; // bytes

    format!(
        r#"resource "libvirt_volume" "disk_{name}" {{
  name             = "{name}.qcow2"
  pool             = "default"
  base_volume_name = "base-ubuntu-{host}.qcow2"
  base_volume_pool = "default"
  size             = {disk_gb}{provider}
}}

resource "libvirt_cloudinit_disk" "init_{name}" {{
  name      = "{name}-init.iso"
  pool      = "default"
  user_data = data.template_file.cloud_init.rendered

  network_config = <<-EOF
version: 2
ethernets:
  ens3:
    addresses:
      - {ip}/24
    gateway4: {gateway}
    nameservers:
      addresses: [8.8.8.8, 8.8.4.4]
EOF{provider}
}}

resource "libvirt_domain" "{name}" {{
  name   = "{name}"
  memory = {mem}
  vcpu   = {vcpu}{provider}

  cloudinit = libvirt_cloudinit_disk.init_{name}.id

  disk {{
    volume_id = libvirt_volume.disk_{name}.id
  }}

  network_interface {{
    bridge         = "{bridge}"
    wait_for_lease = false
  }}{xml_block}
}}
"#,
        ip = node.ip,
        gateway = gateway,
    )
}

fn generate_outputs(spec: &SdiSpec) -> String {
    let mut out = String::from("# --- Outputs ---\n");
    for pool in &spec.spec.sdi_pools {
        for node in &pool.node_specs {
            out.push_str(&format!(
                r#"output "{name}_ip" {{
  value = "{ip}"
}}
"#,
                name = node.node_name,
                ip = node.ip,
            ));
        }
    }
    out
}

/// Generate VFIO passthrough XSLT file content.
/// Pure function.
pub fn generate_vfio_xslt() -> String {
    r#"<?xml version="1.0"?>
<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="xml" indent="yes"/>
  <xsl:template match="@*|node()">
    <xsl:copy>
      <xsl:apply-templates select="@*|node()"/>
    </xsl:copy>
  </xsl:template>
  <!-- Add IOMMU driver for VFIO passthrough -->
  <xsl:template match="/domain/features">
    <xsl:copy>
      <xsl:apply-templates select="@*|node()"/>
      <ioapic driver='qemu'/>
    </xsl:copy>
  </xsl:template>
</xsl:stylesheet>
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::sdi::*;

    fn make_test_spec() -> SdiSpec {
        SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "test".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "192.168.88.0/24".to_string(),
                    gateway: "192.168.88.1".to_string(),
                    nameservers: vec!["8.8.8.8".to_string()],
                },
            },
            os_image: OsImageConfig {
                source: "https://example.com/image.img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "~/.ssh/id.pub".to_string(),
                packages: vec!["curl".to_string()],
            },
            spec: SdiPoolsSpec {
                sdi_pools: vec![SdiPool {
                    pool_name: "tower".to_string(),
                    purpose: "management".to_string(),
                    placement: PlacementConfig {
                        hosts: vec!["playbox-0".to_string()],
                        spread: false,
                    },
                    node_specs: vec![NodeSpec {
                        node_name: "tower-cp-0".to_string(),
                        ip: "192.168.88.100".to_string(),
                        cpu: 2,
                        mem_gb: 3,
                        disk_gb: 30,
                        host: None,
                        roles: vec!["control-plane".to_string()],
                        devices: None,
                    }],
                }],
            },
        }
    }

    #[test]
    fn test_generate_tofu_contains_provider() {
        let spec = make_test_spec();
        let hcl = generate_tofu_main(&spec, "root");
        assert!(hcl.contains("provider \"libvirt\""));
        assert!(hcl.contains("playbox-0"));
    }

    #[test]
    fn test_generate_tofu_contains_vm() {
        let spec = make_test_spec();
        let hcl = generate_tofu_main(&spec, "root");
        assert!(hcl.contains("libvirt_domain"));
        assert!(hcl.contains("tower-cp-0"));
        assert!(hcl.contains("memory = 3072"));
        assert!(hcl.contains("vcpu   = 2"));
    }

    #[test]
    fn test_collect_unique_hosts() {
        let spec = make_test_spec();
        let hosts = collect_unique_hosts(&spec);
        assert_eq!(hosts, vec!["playbox-0"]);
    }

    #[test]
    fn test_generate_tofu_uses_spec_gateway() {
        let mut spec = make_test_spec();
        spec.resource_pool.network.gateway = "10.0.0.1".to_string();
        let hcl = generate_tofu_main(&spec, "root");
        // Gateway must come from spec, not hardcoded
        assert!(hcl.contains("gateway4: 10.0.0.1"));
    }

    #[test]
    fn test_generate_vfio_xslt() {
        let xslt = generate_vfio_xslt();
        assert!(xslt.contains("ioapic"));
        assert!(xslt.contains("xsl:stylesheet"));
    }

    #[test]
    fn test_generate_tofu_host_infra_single_node() {
        let hosts = vec![HostInfraInput {
            name: "playbox-0".to_string(),
            ip: "192.168.88.8".to_string(),
            ssh_user: "admin".to_string(),
        }];
        let hcl = generate_tofu_host_infra(&hosts, "br0", "192.168.88.0/24", "192.168.88.1");

        // Must have terraform block with libvirt provider
        assert!(
            hcl.contains("required_providers"),
            "missing terraform block"
        );
        assert!(
            hcl.contains("dmacvicar/libvirt"),
            "missing libvirt provider source"
        );

        // Must have provider for the host
        assert!(
            hcl.contains("provider \"libvirt\""),
            "missing provider block"
        );
        assert!(
            hcl.contains("qemu+ssh://admin@192.168.88.8/system"),
            "SSH URI must use ssh_user from HostInfraInput"
        );

        // Must create a libvirt storage pool on the host
        assert!(
            hcl.contains("libvirt_pool"),
            "missing libvirt_pool resource"
        );
        assert!(hcl.contains("scalex-pool"), "missing pool name");

        // Must create output for resource pool status
        assert!(hcl.contains("output"), "missing output block");
    }

    #[test]
    fn test_generate_tofu_host_infra_multi_node() {
        let hosts = vec![
            HostInfraInput {
                name: "playbox-0".to_string(),
                ip: "192.168.88.8".to_string(),
                ssh_user: "jinwang".to_string(),
            },
            HostInfraInput {
                name: "playbox-1".to_string(),
                ip: "192.168.88.9".to_string(),
                ssh_user: "jinwang".to_string(),
            },
            HostInfraInput {
                name: "playbox-2".to_string(),
                ip: "192.168.88.10".to_string(),
                ssh_user: "jinwang".to_string(),
            },
            HostInfraInput {
                name: "playbox-3".to_string(),
                ip: "192.168.88.11".to_string(),
                ssh_user: "jinwang".to_string(),
            },
        ];
        let hcl = generate_tofu_host_infra(&hosts, "br0", "192.168.88.0/24", "192.168.88.1");

        // All 4 hosts must have providers with correct ssh_user
        for host in &hosts {
            assert!(
                hcl.contains(&format!("qemu+ssh://{}@{}/system", host.ssh_user, host.ip)),
                "missing provider for {} (ip: {})",
                host.name,
                host.ip
            );
        }

        // All 4 hosts must have storage pool resources
        assert_eq!(
            hcl.matches("resource \"libvirt_pool\"").count(),
            4,
            "expected 4 libvirt_pool resource blocks"
        );
    }

    #[test]
    fn test_generate_tofu_main_uses_admin_user_not_root() {
        let spec = make_test_spec();
        let hcl = generate_tofu_main(&spec, "jinwang");
        // Must use admin_user, not hardcoded root
        assert!(
            hcl.contains("qemu+ssh://jinwang@playbox-0/system?no_verify=1"),
            "SSH URI must use admin_user 'jinwang', not 'root'. Got:\n{}",
            hcl
        );
        assert!(
            !hcl.contains("root@"),
            "SSH URI must NOT contain hardcoded 'root@'"
        );
    }

    #[test]
    fn test_generate_tofu_host_infra_uses_ssh_user() {
        let hosts = vec![HostInfraInput {
            name: "playbox-0".to_string(),
            ip: "192.168.88.8".to_string(),
            ssh_user: "jinwang".to_string(),
        }];
        let hcl = generate_tofu_host_infra(&hosts, "br0", "192.168.88.0/24", "192.168.88.1");
        assert!(
            hcl.contains("qemu+ssh://jinwang@192.168.88.8/system"),
            "SSH URI must use ssh_user 'jinwang', not 'root'"
        );
        assert!(
            !hcl.contains("root@"),
            "SSH URI must NOT contain hardcoded 'root@'"
        );
    }

    #[test]
    fn test_generate_tofu_host_infra_idempotent() {
        let hosts = vec![
            HostInfraInput {
                name: "playbox-0".to_string(),
                ip: "192.168.88.8".to_string(),
                ssh_user: "jinwang".to_string(),
            },
            HostInfraInput {
                name: "playbox-1".to_string(),
                ip: "192.168.88.9".to_string(),
                ssh_user: "jinwang".to_string(),
            },
        ];
        let hcl1 = generate_tofu_host_infra(&hosts, "br0", "192.168.88.0/24", "192.168.88.1");
        let hcl2 = generate_tofu_host_infra(&hosts, "br0", "192.168.88.0/24", "192.168.88.1");
        assert_eq!(hcl1, hcl2, "generate_tofu_host_infra must be deterministic");
    }

    /// CL-1: Single-node SDI — all pools on one host must produce deduplicated provider.
    #[test]
    fn test_single_node_sdi_all_pools_on_one_host() {
        let spec = SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "single-node-pool".to_string(),
                network: NetworkConfig {
                    management_bridge: "br0".to_string(),
                    management_cidr: "192.168.88.0/24".to_string(),
                    gateway: "192.168.88.1".to_string(),
                    nameservers: vec!["8.8.8.8".to_string()],
                },
            },
            os_image: OsImageConfig {
                source: "https://example.com/image.img".to_string(),
                format: "qcow2".to_string(),
            },
            cloud_init: CloudInitConfig {
                ssh_authorized_keys_file: "~/.ssh/id.pub".to_string(),
                packages: vec!["curl".to_string()],
            },
            spec: SdiPoolsSpec {
                sdi_pools: vec![
                    SdiPool {
                        pool_name: "tower".to_string(),
                        purpose: "management".to_string(),
                        placement: PlacementConfig {
                            hosts: vec!["single-node".to_string()],
                            spread: false,
                        },
                        node_specs: vec![NodeSpec {
                            node_name: "tower-cp-0".to_string(),
                            ip: "192.168.88.100".to_string(),
                            cpu: 2,
                            mem_gb: 4,
                            disk_gb: 30,
                            host: None,
                            roles: vec!["control-plane".to_string(), "worker".to_string()],
                            devices: None,
                        }],
                    },
                    SdiPool {
                        pool_name: "sandbox".to_string(),
                        purpose: "workload".to_string(),
                        placement: PlacementConfig {
                            hosts: vec!["single-node".to_string()],
                            spread: false,
                        },
                        node_specs: vec![
                            NodeSpec {
                                node_name: "sandbox-cp-0".to_string(),
                                ip: "192.168.88.110".to_string(),
                                cpu: 2,
                                mem_gb: 4,
                                disk_gb: 40,
                                host: None,
                                roles: vec!["control-plane".to_string()],
                                devices: None,
                            },
                            NodeSpec {
                                node_name: "sandbox-w-0".to_string(),
                                ip: "192.168.88.120".to_string(),
                                cpu: 4,
                                mem_gb: 8,
                                disk_gb: 60,
                                host: None,
                                roles: vec!["worker".to_string()],
                                devices: None,
                            },
                        ],
                    },
                ],
            },
        };

        let hcl = generate_tofu_main(&spec, "admin");

        // Only 1 provider block (deduplicated for single host)
        let provider_count = hcl.matches("provider \"libvirt\"").count();
        assert_eq!(
            provider_count, 1,
            "Single-node SDI must have exactly 1 provider, got {}",
            provider_count
        );

        // Must use correct ssh_user
        assert!(
            hcl.contains("qemu+ssh://admin@single-node/system?no_verify=1"),
            "SSH URI must use provided ssh_user"
        );

        // All 3 VMs must be present
        assert!(hcl.contains("tower-cp-0"), "missing tower CP VM");
        assert!(hcl.contains("sandbox-cp-0"), "missing sandbox CP VM");
        assert!(hcl.contains("sandbox-w-0"), "missing sandbox worker VM");

        // Base volumes are pre-created via virsh, HCL references them by name
        assert!(
            hcl.contains("base_volume_name = \"base-ubuntu-single-node.qcow2\""),
            "All VMs must reference base volume by name"
        );
    }

    /// CL-1: Single-node host infra — exactly 1 provider + 1 pool.
    #[test]
    fn test_single_node_host_infra() {
        let hosts = vec![HostInfraInput {
            name: "solo".to_string(),
            ip: "10.0.0.1".to_string(),
            ssh_user: "admin".to_string(),
        }];
        let hcl = generate_tofu_host_infra(&hosts, "br0", "10.0.0.0/24", "10.0.0.1");

        assert_eq!(
            hcl.matches("provider \"libvirt\"").count(),
            1,
            "Single-node host infra must have exactly 1 provider"
        );
        assert_eq!(
            hcl.matches("resource \"libvirt_pool\"").count(),
            1,
            "Single-node host infra must have exactly 1 storage pool"
        );
        assert!(
            hcl.contains("qemu+ssh://admin@10.0.0.1/system"),
            "Must use correct ssh_user and IP"
        );
    }
}
