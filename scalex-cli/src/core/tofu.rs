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
    // Uses hostname alias (e.g., "playbox-0") instead of raw IP so that
    // the Go SSH client reads ~/.ssh/config for Port, ProxyJump, etc.
    for host in hosts {
        hcl.push_str(&format!(
            r#"provider "libvirt" {{
  alias = "{name}"
  uri   = "qemu+ssh://{ssh_user}@{name}/system?no_verify=1"
}}
"#,
            name = host.name,
            ssh_user = host.ssh_user,
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

    // Extract CIDR prefix from management_cidr (e.g., "192.168.88.0/24" -> "24")
    let cidr_prefix = spec
        .resource_pool
        .network
        .management_cidr
        .rsplit('/')
        .next()
        .unwrap_or("24");

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
                cidr_prefix,
                &spec.resource_pool.network.nameservers,
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

fn generate_base_volume(_host: &str, _source: &str, _format: &str) -> String {
    r#"# Base volume is pre-created via virsh (SSH upload avoids provider timeout)
# Referenced by name in disk volumes via base_volume_name

"#
    .to_string()
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
  - systemctl stop apparmor || true
  - systemctl disable apparmor || true
  - aa-teardown || true
  - mkdir -p /opt/cni/bin
  - chmod 777 /opt/cni/bin
  - apt-get purge -y apparmor 2>/dev/null || true
EOF
}}
"#,
        ssh_key = spec.cloud_init.ssh_authorized_keys_file,
    )
}

fn generate_vm_resource(
    node: &NodeSpec,
    host: &str,
    bridge: &str,
    gateway: &str,
    cidr_prefix: &str,
    nameservers: &[String],
) -> String {
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
      - {ip}/{cidr_prefix}
    routes:
      - to: default
        via: {gateway}
    nameservers:
      addresses: [{nameservers_str}]
EOF{provider}
}}

resource "libvirt_domain" "{name}" {{
  name      = "{name}"
  memory    = {mem}
  vcpu      = {vcpu}
  autostart = true{provider}

  cpu {{
    mode = "host-passthrough"
  }}

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
        cidr_prefix = cidr_prefix,
        nameservers_str = nameservers.join(", "),
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
        assert!(hcl.contains("memory    = 3072"));
        assert!(hcl.contains("vcpu      = 2"));
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
        assert!(hcl.contains("via: 10.0.0.1"));
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
            hcl.contains("qemu+ssh://admin@playbox-0/system"),
            "SSH URI must use hostname alias, not IP"
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
                hcl.contains(&format!("qemu+ssh://{}@{}/system", host.ssh_user, host.name)),
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
            hcl.contains("qemu+ssh://jinwang@playbox-0/system"),
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

    /// AC 8c: 4-host production spec (playbox-0/1/2/3) → generate_tofu_main produces:
    ///  - provider blocks for ALL 4 hosts
    ///  - sandbox-worker-2 VM resources (playbox-3 host)
    ///  - No regression in playbox-0/1/2 VMs
    #[test]
    fn test_generate_tofu_main_4host_playbox3_no_regression() {
        let spec = make_4host_production_spec();
        let hcl = generate_tofu_main(&spec, "jinwang");

        // All 4 provider aliases must exist
        for host in &["playbox-0", "playbox-1", "playbox-2", "playbox-3"] {
            assert!(
                hcl.contains(&format!("alias = \"{}\"", host)),
                "Missing libvirt provider alias for {}",
                host
            );
            assert!(
                hcl.contains(&format!("qemu+ssh://jinwang@{}/system?no_verify=1", host)),
                "Missing SSH URI for {}",
                host
            );
        }

        // New node: sandbox-worker-2 on playbox-3 must appear
        assert!(
            hcl.contains("sandbox-worker-2"),
            "HCL must contain sandbox-worker-2 VM"
        );
        assert!(
            hcl.contains("192.168.88.122"),
            "HCL must contain sandbox-worker-2 IP 192.168.88.122"
        );
        assert!(
            hcl.contains("provider = libvirt.playbox-3"),
            "sandbox-worker-2 must use playbox-3 provider"
        );

        // No regression — existing VMs on playbox-0/1/2 still present
        for node in &[
            "tower-cp-0",
            "tower-cp-1",
            "tower-cp-2",
            "sandbox-cp-0",
            "sandbox-worker-0",
            "sandbox-worker-1",
        ] {
            assert!(
                hcl.contains(node),
                "Regression: missing existing VM {}",
                node
            );
        }

        // IPs for existing nodes unchanged
        for ip in &[
            "192.168.88.100",
            "192.168.88.101",
            "192.168.88.102",
            "192.168.88.110",
            "192.168.88.120",
            "192.168.88.121",
        ] {
            assert!(
                hcl.contains(ip),
                "Regression: existing node IP {} missing from HCL",
                ip
            );
        }

        // Output block must include sandbox-worker-2
        assert!(
            hcl.contains("sandbox-worker-2_ip"),
            "HCL outputs must include sandbox-worker-2_ip"
        );

        // Provider deduplication: each host appears exactly once as alias
        assert_eq!(
            hcl.matches("alias = \"playbox-3\"").count(),
            1,
            "playbox-3 provider alias must appear exactly once (dedup check)"
        );

        // collect_unique_hosts must include all 4
        let hosts = collect_unique_hosts(&spec);
        assert_eq!(hosts.len(), 4, "Must have exactly 4 unique hosts");
        assert!(
            hosts.contains(&"playbox-3".to_string()),
            "playbox-3 must be in unique hosts"
        );
    }

    /// Helper: build the production 4-host spec matching config/sdi-specs.yaml topology.
    fn make_4host_production_spec() -> SdiSpec {
        let node = |name: &str, ip: &str, cpu: u32, mem: u32, disk: u32, host: &str| NodeSpec {
            node_name: name.to_string(),
            ip: ip.to_string(),
            cpu,
            mem_gb: mem,
            disk_gb: disk,
            host: Some(host.to_string()),
            roles: vec!["worker".to_string()],
            devices: None,
        };
        SdiSpec {
            resource_pool: ResourcePoolConfig {
                name: "playbox-pool".to_string(),
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
                ssh_authorized_keys_file: "~/.ssh/id_ed25519.pub".to_string(),
                packages: vec!["curl".to_string()],
            },
            spec: SdiPoolsSpec {
                sdi_pools: vec![
                    SdiPool {
                        pool_name: "tower".to_string(),
                        purpose: "management".to_string(),
                        placement: PlacementConfig {
                            hosts: vec![],
                            spread: true,
                        },
                        node_specs: vec![
                            node("tower-cp-0", "192.168.88.100", 4, 6, 30, "playbox-0"),
                            node("tower-cp-1", "192.168.88.101", 4, 6, 30, "playbox-1"),
                            node("tower-cp-2", "192.168.88.102", 4, 6, 30, "playbox-2"),
                        ],
                    },
                    SdiPool {
                        pool_name: "sandbox".to_string(),
                        purpose: "workload".to_string(),
                        placement: PlacementConfig {
                            hosts: vec![],
                            spread: true,
                        },
                        node_specs: vec![
                            node("sandbox-cp-0", "192.168.88.110", 4, 8, 60, "playbox-0"),
                            node("sandbox-worker-0", "192.168.88.120", 2, 4, 40, "playbox-1"),
                            node("sandbox-worker-1", "192.168.88.121", 2, 4, 40, "playbox-2"),
                            node("sandbox-worker-2", "192.168.88.122", 4, 8, 60, "playbox-3"),
                        ],
                    },
                ],
            },
        }
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
            hcl.contains("qemu+ssh://admin@solo/system"),
            "Must use correct ssh_user and hostname"
        );
    }
}
