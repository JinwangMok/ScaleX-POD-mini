use crate::dash::app::App;
use crate::dash::infra::SdiVmInfo;
use crate::dash::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// Data models for infra view rendering
// ---------------------------------------------------------------------------

/// A single VM card with static specs and optional K8s metrics.
pub(super) struct VmCard {
    pub name: String,
    #[allow(dead_code)]
    pub ip: String,
    pub status: String,
    pub cpu: u32,
    pub mem_gb: u32,
    pub disk_gb: u32,
    pub gpu: bool,
    /// K8s CPU usage percent (None = no metrics / not matched)
    pub k8s_cpu_percent: Option<f64>,
    /// K8s MEM usage percent (None = no metrics / not matched)
    pub k8s_mem_percent: Option<f64>,
    /// Which cluster this VM belongs to (by IP match)
    pub cluster_name: Option<String>,
    /// Whether this VM matches the current pool filter
    pub in_selected_pool: bool,
    /// Index in the filtered VM list (for cursor highlight). usize::MAX if not in pool.
    pub flat_index: usize,
}

/// A bare-metal host box grouping VMs.
pub(super) struct HostBox {
    host: String,
    total_cpu: u32,
    total_mem_gb: u32,
    used_cpu_percent: Option<f64>,
    used_mem_percent: Option<f64>,
    pub vms: Vec<VmCard>,
    /// Whether any VM in this box matches the pool filter
    has_active_vms: bool,
}

// ---------------------------------------------------------------------------
// Data collection (pure function)
// ---------------------------------------------------------------------------

/// Collect and group VMs by bare-metal host, enriching with K8s metrics.
pub(super) fn collect_host_boxes(app: &App) -> Vec<HostBox> {
    let infra = &app.infra;
    if infra.sdi_pools.is_empty() {
        return Vec::new();
    }

    // Build a flat list of all VMs with their pool membership
    struct VmEntry<'a> {
        vm: &'a SdiVmInfo,
        pool_name: String,
    }

    let mut all_vms: Vec<VmEntry> = Vec::new();
    for pool in &infra.sdi_pools {
        for vm in &pool.nodes {
            all_vms.push(VmEntry {
                vm,
                pool_name: pool.pool_name.clone(),
            });
        }
    }

    // Group by host
    let mut host_map: std::collections::BTreeMap<String, Vec<&VmEntry>> =
        std::collections::BTreeMap::new();
    for entry in &all_vms {
        host_map
            .entry(entry.vm.host.clone())
            .or_default()
            .push(entry);
    }

    // Build VM cards with K8s metrics join
    // flat_index counts only in-pool VMs so cursor matches filtered set
    let mut flat_index: usize = 0;
    let mut host_boxes: Vec<HostBox> = Vec::new();

    for (host_name, entries) in &host_map {
        let mut total_cpu: u32 = 0;
        let mut total_mem_gb: u32 = 0;
        let mut cpu_usage_sum: f64 = 0.0;
        let mut mem_usage_sum: f64 = 0.0;
        let mut has_any_metrics = false;
        let mut has_active_vms = false;
        let mut vms: Vec<VmCard> = Vec::new();

        for entry in entries {
            let vm = entry.vm;
            total_cpu += vm.cpu;
            total_mem_gb += vm.mem_gb;

            let in_selected_pool = app
                .selected_sdi_pool
                .as_ref()
                .is_none_or(|sel| &entry.pool_name == sel);
            if in_selected_pool {
                has_active_vms = true;
            }

            // Match VM IP to K8s node across all cluster snapshots
            let mut k8s_cpu: Option<f64> = None;
            let mut k8s_mem: Option<f64> = None;
            let mut cluster: Option<String> = None;

            if !vm.ip.is_empty() {
                for snap in &app.snapshots {
                    if let Some(node) = snap.nodes.iter().find(|n| n.internal_ip == vm.ip) {
                        k8s_cpu = node.cpu_usage_percent;
                        k8s_mem = node.mem_usage_percent;
                        cluster = Some(snap.name.clone());
                        break;
                    }
                }
            }

            // Accumulate for host-level summary
            if let (Some(cpu_pct), Some(mem_pct)) = (k8s_cpu, k8s_mem) {
                // Convert node % back to absolute for host-level aggregation
                cpu_usage_sum += cpu_pct * vm.cpu as f64 / 100.0;
                mem_usage_sum += mem_pct * vm.mem_gb as f64 / 100.0;
                has_any_metrics = true;
            }

            let vm_flat_index = if in_selected_pool {
                let idx = flat_index;
                flat_index += 1;
                idx
            } else {
                usize::MAX // not selectable
            };

            vms.push(VmCard {
                name: vm.name.clone(),
                ip: vm.ip.clone(),
                status: vm.status.clone(),
                cpu: vm.cpu,
                mem_gb: vm.mem_gb,
                disk_gb: vm.disk_gb,
                gpu: vm.gpu,
                k8s_cpu_percent: k8s_cpu,
                k8s_mem_percent: k8s_mem,
                cluster_name: cluster,
                in_selected_pool,
                flat_index: vm_flat_index,
            });
        }

        let (used_cpu_percent, used_mem_percent) = if has_any_metrics && total_cpu > 0 {
            (
                Some(cpu_usage_sum / total_cpu as f64 * 100.0),
                Some(if total_mem_gb > 0 {
                    mem_usage_sum / total_mem_gb as f64 * 100.0
                } else {
                    0.0
                }),
            )
        } else {
            (None, None)
        };

        host_boxes.push(HostBox {
            host: host_name.clone(),
            total_cpu,
            total_mem_gb,
            used_cpu_percent,
            used_mem_percent,
            vms,
            has_active_vms,
        });
    }

    host_boxes
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the SDI infrastructure visualization as a 2x2 grid of bare-metal host boxes.
pub fn render_infra_view(f: &mut Frame, app: &App, area: Rect) {
    let host_boxes = collect_host_boxes(app);

    if host_boxes.is_empty() {
        let msg = Paragraph::new("  No SDI infrastructure data. Run 'scalex sdi init' first.")
            .style(Style::default().fg(theme::FG4));
        f.render_widget(msg, area);
        return;
    }

    // Split into 2x2 grid (or 1x1, 1x2, 2x2 depending on count)
    let (rows, cols) = match host_boxes.len() {
        1 => (1, 1),
        2 => (1, 2),
        3 | 4 => (2, 2),
        _ => (2, 2), // cap at 4 visible
    };

    let row_constraints: Vec<Constraint> = (0..rows)
        .map(|_| Constraint::Ratio(1, rows as u32))
        .collect();
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);

    let mut box_idx = 0;
    for row in 0..rows {
        let col_constraints: Vec<Constraint> = (0..cols)
            .map(|_| Constraint::Ratio(1, cols as u32))
            .collect();
        let col_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(row_areas[row]);

        for col in 0..cols {
            if box_idx < host_boxes.len() {
                render_host_box(f, app, &host_boxes[box_idx], col_areas[col]);
            }
            box_idx += 1;
        }
    }
}

/// Render a single bare-metal host box with nested VM cards.
fn render_host_box(f: &mut Frame, app: &App, hbox: &HostBox, area: Rect) {
    let border_color = if hbox.has_active_vms {
        theme::BRIGHT_BLUE
    } else {
        theme::BG3
    };

    let title = format!(" {} ", hbox.host);
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme::BRIGHT_AQUA)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme::BG));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 10 {
        return;
    }

    // Summary lines: Total and Used
    let total_line = format!(" Total: {}c / {}Gi", hbox.total_cpu, hbox.total_mem_gb);
    let used_line = match (hbox.used_cpu_percent, hbox.used_mem_percent) {
        (Some(cpu), Some(mem)) => format!(" Used:  {:.0}% CPU / {:.0}% MEM", cpu, mem),
        _ => " Used:  N/A".to_string(),
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        total_line,
        Style::default().fg(theme::FG2),
    )));
    lines.push(Line::from(Span::styled(
        used_line,
        Style::default().fg(theme::FG3),
    )));

    // VM cards
    let max_name_width = inner.width.saturating_sub(4) as usize; // padding for borders

    for vm in &hbox.vms {
        // Separator line between summary and first VM
        if lines.len() == 2 {
            lines.push(Line::from(""));
        }

        let style = if !vm.in_selected_pool {
            Style::default().fg(theme::BG3) // dimmed
        } else if vm.flat_index == app.infra_vm_cursor {
            Style::default()
                .fg(theme::BRIGHT_YELLOW)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::FG)
        };

        let status_dot = match vm.status.as_str() {
            "running" => Span::styled("●", Style::default().fg(theme::BRIGHT_GREEN)),
            "paused" => Span::styled("●", Style::default().fg(theme::BRIGHT_YELLOW)),
            _ => Span::styled("●", Style::default().fg(theme::BRIGHT_RED)),
        };

        // Apply dimming override for filtered-out VMs
        let status_dot = if !vm.in_selected_pool {
            Span::styled("●", Style::default().fg(theme::BG3))
        } else {
            status_dot
        };

        // Line 1: VM name + status
        let name_display = if vm.name.len() > max_name_width.saturating_sub(15) {
            let trunc = max_name_width.saturating_sub(18);
            let truncated: String = vm.name.chars().take(trunc).collect();
            format!(" {}...", truncated)
        } else {
            format!(" {}", vm.name)
        };

        let status_label = format!(" [{}] ", vm.status);
        let mut name_spans = vec![
            Span::styled(name_display, style),
            Span::styled(
                status_label,
                if vm.in_selected_pool {
                    Style::default().fg(theme::FG4)
                } else {
                    Style::default().fg(theme::BG3)
                },
            ),
            status_dot,
        ];

        // GPU indicator
        if vm.gpu {
            name_spans.push(Span::styled(
                " GPU",
                if vm.in_selected_pool {
                    Style::default()
                        .fg(theme::BRIGHT_PURPLE)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::BG3)
                },
            ));
        }

        lines.push(Line::from(name_spans));

        // Line 2: specs + K8s metrics
        let specs = format!(" {}c {}Gi {}Gi", vm.cpu, vm.mem_gb, vm.disk_gb);
        let metrics = match (vm.k8s_cpu_percent, vm.k8s_mem_percent) {
            (Some(cpu), Some(mem)) => format!("  CPU:{:.0}% M:{:.0}%", cpu, mem),
            _ => "  N/A".to_string(),
        };

        let cluster_suffix = vm
            .cluster_name
            .as_ref()
            .map(|c| format!(" ({})", c))
            .unwrap_or_default();

        let detail_style = if !vm.in_selected_pool {
            Style::default().fg(theme::BG3)
        } else {
            Style::default().fg(theme::FG3)
        };

        lines.push(Line::from(vec![
            Span::styled(specs, detail_style),
            Span::styled(
                metrics,
                if vm.in_selected_pool {
                    match (vm.k8s_cpu_percent, vm.k8s_mem_percent) {
                        (Some(_), Some(_)) => Style::default().fg(theme::BRIGHT_GREEN),
                        _ => Style::default().fg(theme::FG4),
                    }
                } else {
                    Style::default().fg(theme::BG3)
                },
            ),
            Span::styled(cluster_suffix, detail_style),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::data::{ClusterSnapshot, HealthStatus, NodeInfo, ResourceUsage};
    use crate::dash::infra::{InfraSnapshot, SdiPoolInfo, SdiVmInfo};

    fn make_infra(pools: Vec<SdiPoolInfo>) -> InfraSnapshot {
        let total_vms = pools.iter().map(|p| p.nodes.len()).sum();
        let running_vms = pools
            .iter()
            .flat_map(|p| &p.nodes)
            .filter(|vm| vm.status == "running")
            .count();
        InfraSnapshot {
            sdi_pools: pools,
            total_vms,
            running_vms,
        }
    }

    fn make_vm(name: &str, host: &str, ip: &str, cpu: u32, mem_gb: u32) -> SdiVmInfo {
        SdiVmInfo {
            name: name.into(),
            ip: ip.into(),
            host: host.into(),
            cpu,
            mem_gb,
            disk_gb: 80,
            status: "running".into(),
            gpu: false,
        }
    }

    fn make_node(name: &str, ip: &str, cpu_pct: Option<f64>, mem_pct: Option<f64>) -> NodeInfo {
        NodeInfo {
            name: name.into(),
            internal_ip: ip.into(),
            cpu_usage_percent: cpu_pct,
            mem_usage_percent: mem_pct,
            cpu_capacity: "8".into(),
            mem_capacity: "16Gi".into(),
            ..Default::default()
        }
    }

    fn make_snapshot(name: &str, nodes: Vec<NodeInfo>) -> ClusterSnapshot {
        ClusterSnapshot {
            name: name.into(),
            health: HealthStatus::Green,
            namespaces: vec![],
            nodes,
            pods: vec![],
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            events: vec![],
            resource_usage: ResourceUsage::default(),
        }
    }

    #[test]
    fn collect_host_boxes_groups_by_host() {
        use crate::dash::kube_client::ClusterClient;
        let mut app = App::new(vec![], 10);
        app.infra = make_infra(vec![SdiPoolInfo {
            pool_name: "tower".into(),
            purpose: "mgmt".into(),
            nodes: vec![
                make_vm("tower-cp-0", "playbox-0", "10.0.0.1", 4, 8),
                make_vm("tower-cp-1", "playbox-1", "10.0.0.2", 4, 8),
            ],
        }]);

        let boxes = collect_host_boxes(&app);
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].host, "playbox-0");
        assert_eq!(boxes[0].vms.len(), 1);
        assert_eq!(boxes[1].host, "playbox-1");
        assert_eq!(boxes[1].vms.len(), 1);
    }

    #[test]
    fn collect_host_boxes_filters_by_pool() {
        use crate::dash::kube_client::ClusterClient;
        let mut app = App::new(vec![], 10);
        app.infra = make_infra(vec![
            SdiPoolInfo {
                pool_name: "tower".into(),
                purpose: "mgmt".into(),
                nodes: vec![make_vm("tower-cp-0", "playbox-0", "10.0.0.1", 4, 8)],
            },
            SdiPoolInfo {
                pool_name: "sandbox".into(),
                purpose: "workload".into(),
                nodes: vec![make_vm("sandbox-w-0", "playbox-0", "10.0.0.3", 4, 8)],
            },
        ]);
        app.selected_sdi_pool = Some("sandbox".into());

        let boxes = collect_host_boxes(&app);
        assert_eq!(boxes.len(), 1); // both VMs on playbox-0
        assert_eq!(boxes[0].vms.len(), 2);
        // sandbox VM should be in_selected_pool, tower VM should not
        let sandbox_vm = boxes[0]
            .vms
            .iter()
            .find(|v| v.name == "sandbox-w-0")
            .unwrap();
        let tower_vm = boxes[0]
            .vms
            .iter()
            .find(|v| v.name == "tower-cp-0")
            .unwrap();
        assert!(sandbox_vm.in_selected_pool);
        assert!(!tower_vm.in_selected_pool);
        assert!(boxes[0].has_active_vms);
    }

    #[test]
    fn vm_to_node_mapping_by_ip() {
        use crate::dash::kube_client::ClusterClient;
        let mut app = App::new(vec![], 10);
        app.infra = make_infra(vec![SdiPoolInfo {
            pool_name: "tower".into(),
            purpose: "mgmt".into(),
            nodes: vec![make_vm("tower-cp-0", "playbox-0", "10.0.0.1", 4, 8)],
        }]);
        app.snapshots = vec![make_snapshot(
            "tower",
            vec![make_node("tower-cp-0", "10.0.0.1", Some(25.0), Some(50.0))],
        )];

        let boxes = collect_host_boxes(&app);
        assert_eq!(boxes.len(), 1);
        let vm = &boxes[0].vms[0];
        assert_eq!(vm.k8s_cpu_percent, Some(25.0));
        assert_eq!(vm.k8s_mem_percent, Some(50.0));
        assert_eq!(vm.cluster_name.as_deref(), Some("tower"));
    }

    #[test]
    fn collect_empty_infra() {
        use crate::dash::kube_client::ClusterClient;
        let app = App::new(vec![], 10);
        let boxes = collect_host_boxes(&app);
        assert!(boxes.is_empty());
    }

    #[test]
    fn vm_no_metrics_shows_none() {
        use crate::dash::kube_client::ClusterClient;
        let mut app = App::new(vec![], 10);
        app.infra = make_infra(vec![SdiPoolInfo {
            pool_name: "tower".into(),
            purpose: "mgmt".into(),
            nodes: vec![make_vm("tower-cp-0", "playbox-0", "10.0.0.1", 4, 8)],
        }]);
        // No snapshots → no metrics match

        let boxes = collect_host_boxes(&app);
        assert_eq!(boxes[0].vms[0].k8s_cpu_percent, None);
        assert_eq!(boxes[0].vms[0].k8s_mem_percent, None);
        assert_eq!(boxes[0].used_cpu_percent, None);
    }

    #[test]
    fn host_box_has_no_active_vms_when_pool_filtered() {
        use crate::dash::kube_client::ClusterClient;
        let mut app = App::new(vec![], 10);
        app.infra = make_infra(vec![SdiPoolInfo {
            pool_name: "tower".into(),
            purpose: "mgmt".into(),
            nodes: vec![make_vm("tower-cp-0", "playbox-0", "10.0.0.1", 4, 8)],
        }]);
        app.selected_sdi_pool = Some("sandbox".into());

        let boxes = collect_host_boxes(&app);
        assert!(!boxes[0].has_active_vms);
    }
}
