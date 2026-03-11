use crate::core::config::{load_baremetal_config, NodeConnectionConfig};
use crate::core::error::ScalexError;
use crate::core::ssh::{build_ssh_command, execute_ssh};
use crate::models::baremetal::*;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct FactsArgs {
    /// Gather facts from a specific host only
    #[arg(long)]
    host: Option<String>,

    /// Gather facts from all configured hosts
    #[arg(long, default_value_t = false)]
    all: bool,

    /// Path to baremetal-init.yaml
    #[arg(long, default_value = "credentials/.baremetal-init.yaml")]
    config: PathBuf,

    /// Path to .env file
    #[arg(long, default_value = "credentials/.env")]
    env_file: PathBuf,

    /// Output directory for facts JSON
    #[arg(long, default_value = "_generated/facts")]
    output_dir: PathBuf,

    /// Dry run — show what would be gathered without SSH
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

pub fn run(args: FactsArgs) -> anyhow::Result<()> {
    let config = load_baremetal_config(&args.config, &args.env_file)?;

    let nodes_to_query: Vec<&NodeConnectionConfig> = if let Some(ref host) = args.host {
        config
            .target_nodes
            .iter()
            .filter(|n| n.name == *host)
            .collect()
    } else if args.all {
        config.target_nodes.iter().collect()
    } else {
        // Default: all nodes
        config.target_nodes.iter().collect()
    };

    if nodes_to_query.is_empty() {
        anyhow::bail!("No matching nodes found in config");
    }

    std::fs::create_dir_all(&args.output_dir)?;

    for node in &nodes_to_query {
        println!("[facts] Gathering facts from {}...", node.name);

        if args.dry_run {
            println!("[dry-run] Would SSH to {} ({})", node.name, node.node_ip);
            continue;
        }

        match gather_node_facts(node, &config.target_nodes) {
            Ok(facts) => {
                let json = serde_json::to_string_pretty(&facts)?;
                let output_path = args.output_dir.join(format!("{}.json", facts.node_name));
                std::fs::write(&output_path, &json)?;
                println!("[facts] {} -> {}", node.name, output_path.display());
            }
            Err(e) => {
                eprintln!("[facts] ERROR on {}: {}", node.name, e);
            }
        }
    }

    println!("[facts] Done.");
    Ok(())
}

/// Gather hardware facts from a single node via SSH.
fn gather_node_facts(
    node: &NodeConnectionConfig,
    all_nodes: &[NodeConnectionConfig],
) -> Result<NodeFacts, ScalexError> {
    let script = build_facts_script();
    let ssh_cmd = build_ssh_command(node, &script, all_nodes)?;
    let output = execute_ssh(&ssh_cmd)?;
    parse_facts_output(&node.name, &output)
}

/// Public wrapper for cross-module access.
pub fn build_facts_script_public() -> String {
    build_facts_script()
}

/// Public wrapper for cross-module access.
pub fn parse_facts_output_public(
    node_name: &str,
    raw: &str,
) -> Result<crate::models::baremetal::NodeFacts, crate::core::error::ScalexError> {
    parse_facts_output(node_name, raw)
}

/// Build the remote shell script that gathers all hardware info.
/// Pure function — returns a string.
fn build_facts_script() -> String {
    r#"echo '---SCALEX_FACTS_START---'
echo "cpu_model=$(lscpu 2>/dev/null | grep 'Model name' | sed 's/.*: *//')"
echo "cpu_cores=$(nproc --all 2>/dev/null || echo 0)"
echo "cpu_threads=$(lscpu 2>/dev/null | grep '^CPU(s):' | awk '{print $2}')"
echo "cpu_arch=$(uname -m)"
echo "mem_total_kb=$(grep MemTotal /proc/meminfo | awk '{print $2}')"
echo "mem_avail_kb=$(grep MemAvailable /proc/meminfo | awk '{print $2}')"
echo "kernel_version=$(uname -r)"

echo '---DISKS---'
lsblk -d -b -n -o NAME,SIZE,TYPE,MODEL 2>/dev/null | grep disk || true

echo '---NICS---'
ip -j link show 2>/dev/null || echo '[]'

echo '---NIC_SPEEDS---'
for iface in $(ls /sys/class/net/ 2>/dev/null | grep -v lo); do
    speed=$(cat /sys/class/net/$iface/speed 2>/dev/null || echo -1)
    driver=$(readlink /sys/class/net/$iface/device/driver 2>/dev/null | awk -F/ '{print $NF}')
    state=$(cat /sys/class/net/$iface/operstate 2>/dev/null || echo unknown)
    echo "$iface|$speed|$driver|$state"
done

echo '---GPUS---'
lspci -nn 2>/dev/null | grep -iE 'VGA|3D|Display' || true

echo '---PCIE---'
lspci -nn 2>/dev/null || true

echo '---IOMMU---'
if [ -d /sys/kernel/iommu_groups ]; then
    for g in $(ls /sys/kernel/iommu_groups/ 2>/dev/null | sort -n); do
        devices=""
        for d in /sys/kernel/iommu_groups/$g/devices/*; do
            devices="$devices $(basename $d)"
        done
        echo "group_${g}:${devices}"
    done
else
    echo "NO_IOMMU"
fi

echo '---BRIDGES---'
brctl show 2>/dev/null | tail -n +2 | awk '{print $1}' | sort -u || ip -j link show type bridge 2>/dev/null | python3 -c "import json,sys; [print(d['ifname']) for d in json.load(sys.stdin)]" 2>/dev/null || true

echo '---BONDS---'
ls /sys/class/net/bonding_masters 2>/dev/null && cat /sys/class/net/bonding_masters 2>/dev/null || true

echo '---KERNEL_PARAMS---'
sysctl net.ipv4.ip_forward net.bridge.bridge-nf-call-iptables net.bridge.bridge-nf-call-ip6tables 2>/dev/null || true

echo '---SCALEX_FACTS_END---'"#
        .to_string()
}

/// Parse the raw SSH output into structured NodeFacts.
/// Pure function.
fn parse_facts_output(node_name: &str, raw: &str) -> Result<NodeFacts, ScalexError> {
    let make_err = |detail: &str| ScalexError::FactsParse {
        host: node_name.to_string(),
        detail: detail.to_string(),
    };

    // Find the facts section
    let start = raw
        .find("---SCALEX_FACTS_START---")
        .ok_or_else(|| make_err("No start marker"))?;
    let end = raw
        .find("---SCALEX_FACTS_END---")
        .ok_or_else(|| make_err("No end marker"))?;
    let body = &raw[start..end];

    let get_val = |prefix: &str| -> String {
        body.lines()
            .find(|l| l.starts_with(prefix))
            .map(|l| l.strip_prefix(prefix).unwrap_or("").to_string())
            .unwrap_or_default()
    };

    let cpu = CpuInfo {
        model: get_val("cpu_model="),
        cores: get_val("cpu_cores=").parse().unwrap_or(0),
        threads: get_val("cpu_threads=").parse().unwrap_or(0),
        architecture: get_val("cpu_arch="),
    };

    let mem_total_kb: u64 = get_val("mem_total_kb=").parse().unwrap_or(0);
    let mem_avail_kb: u64 = get_val("mem_avail_kb=").parse().unwrap_or(0);
    let memory = MemoryInfo {
        total_mb: mem_total_kb / 1024,
        available_mb: mem_avail_kb / 1024,
    };

    let kernel_version = get_val("kernel_version=");

    // Parse disks
    let disks = parse_section(body, "---DISKS---", "---NICS---")
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            if parts.len() >= 3 {
                let size_bytes: u64 = parts[1].parse().unwrap_or(0);
                Some(DiskInfo {
                    name: parts[0].to_string(),
                    size_gb: size_bytes / 1_073_741_824,
                    disk_type: if parts.len() > 2 {
                        parts[2].to_string()
                    } else {
                        "unknown".to_string()
                    },
                    model: if parts.len() > 3 {
                        parts[3..].join(" ")
                    } else {
                        String::new()
                    },
                })
            } else {
                None
            }
        })
        .collect();

    // Parse NIC speeds
    let nic_section = parse_section(body, "---NIC_SPEEDS---", "---GPUS---");
    let nics = nic_section
        .lines()
        .filter(|l| !l.is_empty() && l.contains('|'))
        .map(|l| {
            let parts: Vec<&str> = l.split('|').collect();
            let speed_mbit: i64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(-1);
            let speed_str = if speed_mbit >= 10000 {
                format!("{}G", speed_mbit / 1000)
            } else if speed_mbit > 0 {
                format!("{}M", speed_mbit)
            } else {
                "unknown".to_string()
            };
            NicInfo {
                name: parts.first().unwrap_or(&"").to_string(),
                mac: String::new(), // Filled from ip link if needed
                speed: speed_str,
                driver: parts.get(2).unwrap_or(&"").to_string(),
                state: parts.get(3).unwrap_or(&"unknown").to_string(),
            }
        })
        .collect();

    // Parse GPUs
    let gpu_section = parse_section(body, "---GPUS---", "---PCIE---");
    let gpus = gpu_section
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let pci_id = l.split_whitespace().next().unwrap_or("").to_string();
            GpuInfo {
                pci_id,
                model: l.to_string(),
                vendor: if l.to_lowercase().contains("nvidia") {
                    "nvidia".to_string()
                } else if l.to_lowercase().contains("amd") {
                    "amd".to_string()
                } else {
                    "unknown".to_string()
                },
                driver: String::new(),
            }
        })
        .collect();

    // Parse IOMMU groups
    let iommu_section = parse_section(body, "---IOMMU---", "---BRIDGES---");
    let iommu_groups = iommu_section
        .lines()
        .filter(|l| l.starts_with("group_"))
        .filter_map(|l| {
            let (id_part, devices_part) = l.split_once(':')?;
            let id: u32 = id_part.strip_prefix("group_")?.parse().ok()?;
            let devices: Vec<String> = devices_part
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            Some(IommuGroup { id, devices })
        })
        .collect();

    // Parse bridges
    let bridge_section = parse_section(body, "---BRIDGES---", "---BONDS---");
    let bridges: Vec<String> = bridge_section
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.trim().to_string())
        .collect();

    // Parse bonds
    let bond_section = parse_section(body, "---BONDS---", "---KERNEL_PARAMS---");
    let bonds: Vec<String> = bond_section
        .lines()
        .filter(|l| !l.is_empty() && !l.contains("No such file"))
        .flat_map(|l| {
            l.split_whitespace()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    // Parse kernel params
    let kparam_section = parse_section(body, "---KERNEL_PARAMS---", "---SCALEX_FACTS_END---");
    let mut kernel_params = std::collections::HashMap::new();
    for line in kparam_section.lines() {
        if let Some((k, v)) = line.split_once(" = ") {
            kernel_params.insert(k.trim().to_string(), v.trim().to_string());
        } else if let Some((k, v)) = line.split_once('=') {
            kernel_params.insert(k.trim().to_string(), v.trim().to_string());
        }
    }

    // Parse PCIe
    let pcie_section = parse_section(body, "---PCIE---", "---IOMMU---");
    let pcie = pcie_section
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let id = l.split_whitespace().next().unwrap_or("").to_string();
            PcieDevice {
                id,
                class: String::new(),
                vendor: String::new(),
                device: l.to_string(),
            }
        })
        .collect();

    let timestamp = chrono::Utc::now().to_rfc3339();

    Ok(NodeFacts {
        node_name: node_name.to_string(),
        timestamp,
        cpu,
        memory,
        disks,
        nics,
        gpus,
        iommu_groups,
        kernel: KernelInfo {
            version: kernel_version,
            params: kernel_params,
        },
        bridges,
        bonds,
        pcie,
    })
}

/// Extract text between two section markers.
fn parse_section(body: &str, start_marker: &str, end_marker: &str) -> String {
    let start = body
        .find(start_marker)
        .map(|i| i + start_marker.len())
        .unwrap_or(0);
    let end = body.find(end_marker).unwrap_or(body.len());
    if start < end {
        body[start..end].trim().to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_facts_output() {
        let raw = r#"some preamble
---SCALEX_FACTS_START---
cpu_model=Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz
cpu_cores=6
cpu_threads=12
cpu_arch=x86_64
mem_total_kb=32768000
mem_avail_kb=28000000
kernel_version=6.8.0-45-generic
---DISKS---
sda 500107862016 disk Samsung_SSD_870
nvme0n1 1000204886016 disk WD_BLACK_SN770
---NICS---
[]
---NIC_SPEEDS---
eno1|1000|e1000e|up
ens2f0|10000|mlx5_core|up
---GPUS---
01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA106 [GeForce RTX 3060]
---PCIE---
00:00.0 Host bridge: Intel stuff
01:00.0 VGA: NVIDIA GA106
---IOMMU---
group_1: 0000:01:00.0 0000:01:00.1
group_2: 0000:00:1f.0
---BRIDGES---
br0
---BONDS---
bond0
---KERNEL_PARAMS---
net.ipv4.ip_forward = 1
net.bridge.bridge-nf-call-iptables = 1
---SCALEX_FACTS_END---
"#;
        let facts = parse_facts_output("test-node", raw).unwrap();
        assert_eq!(facts.node_name, "test-node");
        assert_eq!(facts.cpu.model, "Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz");
        assert_eq!(facts.cpu.cores, 6);
        assert_eq!(facts.cpu.threads, 12);
        assert_eq!(facts.memory.total_mb, 32000);
        assert_eq!(facts.disks.len(), 2);
        assert_eq!(facts.disks[0].name, "sda");
        assert_eq!(facts.nics.len(), 2);
        assert_eq!(facts.nics[0].name, "eno1");
        assert_eq!(facts.nics[1].speed, "10G");
        assert_eq!(facts.gpus.len(), 1);
        assert_eq!(facts.gpus[0].vendor, "nvidia");
        assert_eq!(facts.iommu_groups.len(), 2);
        assert_eq!(facts.iommu_groups[0].devices.len(), 2);
        assert_eq!(facts.bridges, vec!["br0"]);
        assert_eq!(facts.bonds, vec!["bond0"]);
        assert_eq!(facts.kernel.params.get("net.ipv4.ip_forward").unwrap(), "1");
    }

    #[test]
    fn test_build_facts_script_not_empty() {
        let script = build_facts_script();
        assert!(script.contains("SCALEX_FACTS_START"));
        assert!(script.contains("SCALEX_FACTS_END"));
        assert!(script.contains("lscpu"));
    }

    /// Checklist #8: facts must gather kernel, params, cpu, mem, gpu, storage, pcie.
    #[test]
    fn test_facts_script_covers_all_required_hardware_sections() {
        let script = build_facts_script();

        let required_sections = vec![
            ("cpu", "cpu_model="),
            ("cpu_cores", "cpu_cores="),
            ("memory", "MemTotal"),
            ("kernel_version", "uname -r"),
            ("storage/disks", "---DISKS---"),
            ("network/nics", "---NICS---"),
            ("gpu", "---GPUS---"),
            ("pcie", "---PCIE---"),
            ("iommu_groups", "---IOMMU---"),
            ("kernel_params", "---KERNEL_PARAMS---"),
            ("bridges", "---BRIDGES---"),
            ("bonds", "---BONDS---"),
        ];

        let mut missing = Vec::new();
        for (category, marker) in &required_sections {
            if !script.contains(marker) {
                missing.push(*category);
            }
        }

        assert!(
            missing.is_empty(),
            "Facts script missing required hardware sections: {:?}",
            missing
        );
    }

    /// Verify parsed facts contain all structured fields from Checklist #8.
    #[test]
    fn test_parsed_facts_has_all_checklist_fields() {
        let raw = r#"---SCALEX_FACTS_START---
cpu_model=TestCPU
cpu_cores=4
cpu_threads=8
cpu_arch=x86_64
mem_total_kb=16384000
mem_avail_kb=12000000
kernel_version=6.8.0-45-generic
---DISKS---
sda 500107862016 disk TestDisk
---NICS---
[]
---NIC_SPEEDS---
eth0|1000|e1000e|up
---GPUS---
01:00.0 VGA compatible controller [0300]: NVIDIA Test GPU
---PCIE---
00:00.0 Host bridge: Test
01:00.0 VGA: NVIDIA
---IOMMU---
group_1: 0000:01:00.0
---BRIDGES---
---BONDS---
---KERNEL_PARAMS---
net.ipv4.ip_forward = 1
---SCALEX_FACTS_END---"#;

        let facts = parse_facts_output("node-0", raw).unwrap();

        // CPU (Checklist: cpu)
        assert!(!facts.cpu.model.is_empty(), "cpu.model must be populated");
        assert!(facts.cpu.cores > 0, "cpu.cores must be > 0");
        assert!(facts.cpu.threads > 0, "cpu.threads must be > 0");
        assert!(
            !facts.cpu.architecture.is_empty(),
            "cpu.architecture must be populated"
        );

        // Memory (Checklist: mem)
        assert!(facts.memory.total_mb > 0, "memory.total_mb must be > 0");

        // Kernel (Checklist: kernel version + params)
        assert!(
            !facts.kernel.version.is_empty(),
            "kernel.version must be populated"
        );
        assert!(
            !facts.kernel.params.is_empty(),
            "kernel.params must be populated"
        );

        // Storage (Checklist: storage)
        assert!(!facts.disks.is_empty(), "disks must be populated");

        // GPU (Checklist: gpu)
        assert!(!facts.gpus.is_empty(), "gpus must be populated");
        assert!(
            !facts.gpus[0].vendor.is_empty(),
            "gpu vendor must be populated"
        );

        // PCIe (Checklist: pcie)
        assert!(!facts.pcie.is_empty(), "pcie devices must be populated");

        // Network
        assert!(!facts.nics.is_empty(), "nics must be populated");
    }

    // --- Sprint 32b: facts module edge case tests ---

    #[test]
    fn test_parse_facts_output_missing_start_marker() {
        let raw = "no markers here\n---SCALEX_FACTS_END---";
        let result = parse_facts_output("node", raw);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("start marker") || err_msg.contains("No start"));
    }

    #[test]
    fn test_parse_facts_output_missing_end_marker() {
        let raw = "---SCALEX_FACTS_START---\ncpu_model=Test\n";
        let result = parse_facts_output("node", raw);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_facts_output_empty_sections() {
        // All sections present but empty — should parse without panic
        let raw = r#"---SCALEX_FACTS_START---
cpu_model=
cpu_cores=0
cpu_threads=0
cpu_arch=
mem_total_kb=0
mem_avail_kb=0
kernel_version=
---DISKS---
---NICS---
[]
---NIC_SPEEDS---
---GPUS---
---PCIE---
---IOMMU---
---BRIDGES---
---BONDS---
---KERNEL_PARAMS---
---SCALEX_FACTS_END---"#;
        let facts = parse_facts_output("empty-node", raw).unwrap();
        assert_eq!(facts.cpu.cores, 0);
        assert!(facts.disks.is_empty());
        assert!(facts.nics.is_empty());
        assert!(facts.gpus.is_empty());
        assert!(facts.pcie.is_empty());
        assert!(facts.iommu_groups.is_empty());
        assert!(facts.bridges.is_empty());
        assert!(facts.kernel.params.is_empty());
    }

    #[test]
    fn test_parse_section_missing_markers() {
        let body = "no markers here";
        // start_marker not found => start=0, end_marker not found => end=body.len()
        let result = parse_section(body, "---NOPE1---", "---NOPE2---");
        assert_eq!(result, "no markers here");
    }

    #[test]
    fn test_parse_section_start_after_end() {
        // end marker appears before start marker => empty string
        let body = "---END---\nstuff\n---START---\nmore";
        let result = parse_section(body, "---START---", "---END---");
        assert_eq!(result, "");
    }

    #[test]
    fn test_nic_speed_formatting_boundaries() {
        // 10000 Mbit -> 10G, 1000 Mbit -> 1000M (not 1G since threshold is 10000)
        let raw = r#"---SCALEX_FACTS_START---
cpu_model=T
cpu_cores=1
cpu_threads=1
cpu_arch=x86_64
mem_total_kb=1024
mem_avail_kb=512
kernel_version=6.0
---DISKS---
---NICS---
[]
---NIC_SPEEDS---
eth0|10000|mlx5|up
eth1|1000|e1000e|up
eth2|100|realtek|up
eth3|-1|veth|down
eth4|25000|mlx5|up
---GPUS---
---PCIE---
---IOMMU---
---BRIDGES---
---BONDS---
---KERNEL_PARAMS---
---SCALEX_FACTS_END---"#;
        let facts = parse_facts_output("nic-test", raw).unwrap();
        assert_eq!(facts.nics.len(), 5);
        assert_eq!(facts.nics[0].speed, "10G");   // 10000 >= 10000
        assert_eq!(facts.nics[1].speed, "1000M");  // 1000 > 0 but < 10000
        assert_eq!(facts.nics[2].speed, "100M");    // 100 > 0
        assert_eq!(facts.nics[3].speed, "unknown"); // -1
        assert_eq!(facts.nics[4].speed, "25G");     // 25000 >= 10000
    }

    #[test]
    fn test_gpu_vendor_detection_amd_and_unknown() {
        let raw = r#"---SCALEX_FACTS_START---
cpu_model=T
cpu_cores=1
cpu_threads=1
cpu_arch=x86_64
mem_total_kb=1024
mem_avail_kb=512
kernel_version=6.0
---DISKS---
---NICS---
[]
---NIC_SPEEDS---
---GPUS---
01:00.0 VGA compatible controller: AMD/ATI Radeon RX 7900 XTX
02:00.0 3D controller: SomeVendor Unknown GPU
---PCIE---
---IOMMU---
---BRIDGES---
---BONDS---
---KERNEL_PARAMS---
---SCALEX_FACTS_END---"#;
        let facts = parse_facts_output("gpu-test", raw).unwrap();
        assert_eq!(facts.gpus.len(), 2);
        assert_eq!(facts.gpus[0].vendor, "amd");
        assert_eq!(facts.gpus[1].vendor, "unknown");
    }

    #[test]
    fn test_iommu_group_parsing_with_multiple_devices() {
        let raw = r#"---SCALEX_FACTS_START---
cpu_model=T
cpu_cores=1
cpu_threads=1
cpu_arch=x86_64
mem_total_kb=1024
mem_avail_kb=512
kernel_version=6.0
---DISKS---
---NICS---
[]
---NIC_SPEEDS---
---GPUS---
---PCIE---
---IOMMU---
group_0: 0000:00:00.0
group_1: 0000:01:00.0 0000:01:00.1 0000:01:00.2
NO_IOMMU
---BRIDGES---
---BONDS---
---KERNEL_PARAMS---
---SCALEX_FACTS_END---"#;
        let facts = parse_facts_output("iommu-test", raw).unwrap();
        assert_eq!(facts.iommu_groups.len(), 2);
        assert_eq!(facts.iommu_groups[0].id, 0);
        assert_eq!(facts.iommu_groups[0].devices.len(), 1);
        assert_eq!(facts.iommu_groups[1].id, 1);
        assert_eq!(facts.iommu_groups[1].devices.len(), 3);
    }

    #[test]
    fn test_kernel_params_both_formats() {
        // sysctl uses " = " separator, but some systems may use "="
        let raw = r#"---SCALEX_FACTS_START---
cpu_model=T
cpu_cores=1
cpu_threads=1
cpu_arch=x86_64
mem_total_kb=1024
mem_avail_kb=512
kernel_version=6.0
---DISKS---
---NICS---
[]
---NIC_SPEEDS---
---GPUS---
---PCIE---
---IOMMU---
---BRIDGES---
---BONDS---
---KERNEL_PARAMS---
net.ipv4.ip_forward = 1
net.bridge.bridge-nf-call-iptables=0
---SCALEX_FACTS_END---"#;
        let facts = parse_facts_output("kp-test", raw).unwrap();
        assert_eq!(facts.kernel.params.get("net.ipv4.ip_forward").unwrap(), "1");
        assert_eq!(
            facts.kernel.params.get("net.bridge.bridge-nf-call-iptables").unwrap(),
            "0"
        );
    }

    #[test]
    fn test_disk_size_conversion_bytes_to_gb() {
        let raw = r#"---SCALEX_FACTS_START---
cpu_model=T
cpu_cores=1
cpu_threads=1
cpu_arch=x86_64
mem_total_kb=1024
mem_avail_kb=512
kernel_version=6.0
---DISKS---
nvme0n1 1000204886016 disk WD_BLACK SN770
sda 0 disk TinyDisk
---NICS---
[]
---NIC_SPEEDS---
---GPUS---
---PCIE---
---IOMMU---
---BRIDGES---
---BONDS---
---KERNEL_PARAMS---
---SCALEX_FACTS_END---"#;
        let facts = parse_facts_output("disk-test", raw).unwrap();
        assert_eq!(facts.disks.len(), 2);
        assert_eq!(facts.disks[0].size_gb, 931); // ~931 GB
        assert_eq!(facts.disks[0].model, "WD_BLACK SN770");
        assert_eq!(facts.disks[1].size_gb, 0); // 0 bytes
    }
}
