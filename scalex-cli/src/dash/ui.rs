use crate::dash::app::{ActivePanel, App, NodeType, ResourceView};
use crate::dash::data::HealthStatus;
use crate::dash::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// Main render
// ---------------------------------------------------------------------------

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();

    // Background
    f.render_widget(
        Block::default().style(Style::default().bg(theme::BG)),
        size,
    );

    // Top-level layout: tab bar (1) | body | status bar (3)
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // tab bar
            Constraint::Min(5),    // body
            Constraint::Length(3), // status bar
        ])
        .split(size);

    render_tab_bar(f, app, vertical[0]);

    // Body: sidebar | center
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(28), // sidebar
            Constraint::Min(20),   // center
        ])
        .split(vertical[1]);

    render_sidebar(f, app, horizontal[0]);
    render_center(f, app, horizontal[1]);
    render_status_bar(f, app, vertical[2]);

    if app.show_help {
        render_help_overlay(f, size);
    }
}

// ---------------------------------------------------------------------------
// Tab bar
// ---------------------------------------------------------------------------

fn render_tab_bar(f: &mut Frame, app: &App, area: Rect) {
    let spans: Vec<Span> = app
        .tabs
        .iter()
        .enumerate()
        .flat_map(|(i, tab)| {
            let num = format!(" [{}] ", i + 1);
            let style = if i == app.active_tab {
                Style::default()
                    .fg(theme::BG_HARD)
                    .bg(theme::BRIGHT_YELLOW)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::FG4).bg(theme::BG1)
            };
            vec![
                Span::styled(num, style),
                Span::styled(&tab.name, style),
                Span::styled(" ", Style::default().bg(theme::BG)),
            ]
        })
        .collect();

    let line = Line::from(spans);
    let bar = Paragraph::new(line).style(Style::default().bg(theme::BG));
    f.render_widget(bar, area);
}

// ---------------------------------------------------------------------------
// Sidebar (NERDTree-style)
// ---------------------------------------------------------------------------

fn render_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == ActivePanel::Sidebar;
    let border_color = if is_active { theme::BRIGHT_YELLOW } else { theme::BG3 };

    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme::BG_HARD));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let visible = app.visible_tree_indices();
    let lines: Vec<Line> = visible
        .iter()
        .enumerate()
        .map(|(vi, &idx)| {
            let node = &app.tree[idx];
            let is_selected = vi == app.tree_cursor && is_active;

            let icon = match &node.node_type {
                NodeType::Root => if node.expanded { "  " } else { "  " },
                NodeType::Cluster(_) => if node.expanded { "  " } else { "  " },
                NodeType::Namespace { .. } => "  ",
                NodeType::InfraHeader => if node.expanded { " 󰒍 " } else { " 󰒍 " },
                NodeType::InfraItem(_) => "  ",
            };

            let indent = "  ".repeat(node.depth);
            let label_color = match &node.node_type {
                NodeType::Root => theme::BRIGHT_ORANGE,
                NodeType::Cluster(_) => theme::BRIGHT_BLUE,
                NodeType::Namespace { .. } => theme::FG,
                NodeType::InfraHeader => theme::BRIGHT_AQUA,
                NodeType::InfraItem(_) => theme::FG3,
            };

            let style = if is_selected {
                Style::default()
                    .fg(theme::BG_HARD)
                    .bg(theme::BRIGHT_YELLOW)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(label_color).bg(theme::BG_HARD)
            };

            Line::from(vec![
                Span::styled(indent, Style::default().bg(theme::BG_HARD)),
                Span::styled(icon, style),
                Span::styled(&node.label, style),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Center panel
// ---------------------------------------------------------------------------

fn render_center(f: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == ActivePanel::Center;
    let border_color = if is_active { theme::BRIGHT_YELLOW } else { theme::BG3 };

    let ctx_label = match (&app.selected_cluster, &app.selected_namespace) {
        (Some(c), Some(ns)) => format!("{} > {}", c, ns),
        (Some(c), None) => format!("{} > All Namespaces", c),
        _ => "No cluster selected".to_string(),
    };

    let title = format!(" {} | {} ", app.resource_view.label(), ctx_label);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme::BG));

    let inner = block.inner(area);
    f.render_widget(block, area);

    match app.active_tab {
        0 => render_resources_tab(f, app, inner),
        1 => render_top_tab(f, app, inner),
        _ => {
            let msg = Paragraph::new("Custom tab (press ? for help)")
                .style(Style::default().fg(theme::FG4));
            f.render_widget(msg, inner);
        }
    }
}

fn render_resources_tab(f: &mut Frame, app: &App, area: Rect) {
    let snapshot = match app.current_snapshot() {
        Some(s) => s,
        None => {
            let msg = Paragraph::new("Select a cluster from the sidebar")
                .style(Style::default().fg(theme::FG4));
            f.render_widget(msg, area);
            return;
        }
    };

    match app.resource_view {
        ResourceView::Pods => render_pods_table(f, app, &snapshot.pods, area),
        ResourceView::Deployments => render_deployments_table(f, &snapshot.deployments, area),
        ResourceView::Services => render_services_table(f, &snapshot.services, area),
        ResourceView::Nodes => render_nodes_table(f, &snapshot.nodes, area),
        ResourceView::ConfigMaps => {
            let msg = Paragraph::new("ConfigMaps view (coming soon)")
                .style(Style::default().fg(theme::FG4));
            f.render_widget(msg, area);
        }
    }
}

fn render_pods_table(
    f: &mut Frame,
    app: &App,
    pods: &[crate::dash::data::PodInfo],
    area: Rect,
) {
    let header = Row::new(vec!["NAME", "NAMESPACE", "STATUS", "READY", "RESTARTS", "AGE", "NODE"])
        .style(Style::default().fg(theme::BRIGHT_YELLOW).add_modifier(Modifier::BOLD))
        .bottom_margin(0);

    let rows: Vec<Row> = pods
        .iter()
        .enumerate()
        .map(|(i, pod)| {
            let status_color = match pod.status.as_str() {
                "Running" => theme::BRIGHT_GREEN,
                "Pending" => theme::BRIGHT_YELLOW,
                "Succeeded" => theme::BRIGHT_BLUE,
                "Failed" | "CrashLoopBackOff" | "Error" => theme::BRIGHT_RED,
                _ => theme::FG3,
            };

            let is_selected = i == app.table_cursor && app.active_panel == ActivePanel::Center;
            let base = if is_selected {
                Style::default().fg(theme::BG_HARD).bg(theme::BRIGHT_YELLOW)
            } else {
                Style::default().fg(theme::FG).bg(theme::BG)
            };

            Row::new(vec![
                Cell::from(pod.name.as_str()).style(base),
                Cell::from(pod.namespace.as_str()).style(base),
                Cell::from(pod.status.as_str()).style(
                    if is_selected { base } else { Style::default().fg(status_color) }
                ),
                Cell::from(pod.ready.as_str()).style(base),
                Cell::from(pod.restarts.to_string()).style(base),
                Cell::from(pod.age.as_str()).style(base),
                Cell::from(pod.node.as_str()).style(base),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(22),
            Constraint::Percentage(14),
            Constraint::Percentage(12),
            Constraint::Percentage(8),
            Constraint::Percentage(10),
            Constraint::Percentage(8),
            Constraint::Percentage(20),
        ],
    )
    .header(header)
    .style(Style::default().bg(theme::BG));

    f.render_widget(table, area);
}

fn render_deployments_table(
    f: &mut Frame,
    deployments: &[crate::dash::data::DeploymentInfo],
    area: Rect,
) {
    let header = Row::new(vec!["NAME", "NAMESPACE", "READY", "UP-TO-DATE", "AVAILABLE", "AGE"])
        .style(Style::default().fg(theme::BRIGHT_YELLOW).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = deployments
        .iter()
        .map(|dep| {
            Row::new(vec![
                Cell::from(dep.name.clone()),
                Cell::from(dep.namespace.clone()),
                Cell::from(dep.ready.clone()),
                Cell::from(dep.up_to_date.to_string()),
                Cell::from(dep.available.to_string()),
                Cell::from(dep.age.clone()),
            ])
            .style(Style::default().fg(theme::FG))
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(25),
            Constraint::Percentage(15),
            Constraint::Percentage(12),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(10),
        ],
    )
    .header(header)
    .style(Style::default().bg(theme::BG));

    f.render_widget(table, area);
}

fn render_services_table(
    f: &mut Frame,
    services: &[crate::dash::data::ServiceInfo],
    area: Rect,
) {
    let header = Row::new(vec!["NAME", "NAMESPACE", "TYPE", "CLUSTER-IP", "PORTS", "AGE"])
        .style(Style::default().fg(theme::BRIGHT_YELLOW).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = services
        .iter()
        .map(|svc| {
            Row::new(vec![
                svc.name.as_str(),
                svc.namespace.as_str(),
                svc.svc_type.as_str(),
                svc.cluster_ip.as_str(),
                svc.ports.as_str(),
                svc.age.as_str(),
            ])
            .style(Style::default().fg(theme::FG))
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(22),
            Constraint::Percentage(14),
            Constraint::Percentage(12),
            Constraint::Percentage(18),
            Constraint::Percentage(20),
            Constraint::Percentage(8),
        ],
    )
    .header(header)
    .style(Style::default().bg(theme::BG));

    f.render_widget(table, area);
}

fn render_nodes_table(f: &mut Frame, nodes: &[crate::dash::data::NodeInfo], area: Rect) {
    let header = Row::new(vec!["NAME", "STATUS", "ROLES", "CPU", "MEMORY"])
        .style(Style::default().fg(theme::BRIGHT_YELLOW).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = nodes
        .iter()
        .map(|node| {
            let status_color = if node.status == "Ready" {
                theme::BRIGHT_GREEN
            } else {
                theme::BRIGHT_RED
            };

            Row::new(vec![
                Cell::from(node.name.clone()).style(Style::default().fg(theme::FG)),
                Cell::from(node.status.clone()).style(Style::default().fg(status_color)),
                Cell::from(node.roles.join(",")).style(Style::default().fg(theme::FG3)),
                Cell::from(format!("{}/{}", node.cpu_allocatable, node.cpu_capacity))
                    .style(Style::default().fg(theme::BRIGHT_AQUA)),
                Cell::from(format!("{}/{}", node.mem_allocatable, node.mem_capacity))
                    .style(Style::default().fg(theme::BRIGHT_PURPLE)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(25),
            Constraint::Percentage(12),
            Constraint::Percentage(18),
            Constraint::Percentage(20),
            Constraint::Percentage(25),
        ],
    )
    .header(header)
    .style(Style::default().bg(theme::BG));

    f.render_widget(table, area);
}

fn render_top_tab(f: &mut Frame, app: &App, area: Rect) {
    let snapshot = match app.current_snapshot() {
        Some(s) => s,
        None => {
            let msg = Paragraph::new("Select a cluster to view resource utilization")
                .style(Style::default().fg(theme::FG4));
            f.render_widget(msg, area);
            return;
        }
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                " Node Resource Utilization ",
                Style::default()
                    .fg(theme::BRIGHT_YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    for node in &snapshot.nodes {
        let status_icon = if node.status == "Ready" { "●" } else { "○" };
        let status_color = if node.status == "Ready" {
            theme::BRIGHT_GREEN
        } else {
            theme::BRIGHT_RED
        };

        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", status_icon), Style::default().fg(status_color)),
            Span::styled(&node.name, Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  CPU: {}/{}  MEM: {}/{}", node.cpu_allocatable, node.cpu_capacity, node.mem_allocatable, node.mem_capacity),
                Style::default().fg(theme::FG3),
            ),
        ]));
    }

    if snapshot.nodes.is_empty() {
        lines.push(Line::from(Span::styled(
            " No node data available",
            Style::default().fg(theme::FG4),
        )));
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(theme::BG));
    f.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme::BG3))
        .style(Style::default().bg(theme::BG_HARD));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Line 1: cluster health indicators
    let mut health_spans: Vec<Span> = vec![Span::styled(
        " Clusters: ",
        Style::default().fg(theme::FG4),
    )];

    for snapshot in &app.snapshots {
        let (symbol, color) = match snapshot.health {
            HealthStatus::Green => ("●", theme::BRIGHT_GREEN),
            HealthStatus::Yellow => ("●", theme::BRIGHT_YELLOW),
            HealthStatus::Red => ("●", theme::BRIGHT_RED),
            HealthStatus::Unknown => ("○", theme::FG4),
        };
        health_spans.push(Span::styled(
            format!("{} ", symbol),
            Style::default().fg(color),
        ));
        health_spans.push(Span::styled(
            format!("{}  ", snapshot.name),
            Style::default().fg(theme::FG3),
        ));
    }

    // Line 2: resource usage + overhead
    let mut usage_spans: Vec<Span> = vec![Span::styled(
        " Usage: ",
        Style::default().fg(theme::FG4),
    )];

    for snapshot in &app.snapshots {
        let ru = &snapshot.resource_usage;
        usage_spans.push(Span::styled(
            format!(
                "{}: pods {}/{} nodes {}/{}  ",
                snapshot.name, ru.running_pods, ru.total_pods, ru.ready_nodes, ru.total_nodes
            ),
            Style::default().fg(theme::FG3),
        ));
    }

    usage_spans.push(Span::styled(
        format!("| latency: {}ms", app.api_latency_ms),
        Style::default().fg(theme::BRIGHT_AQUA),
    ));

    let status_text = vec![
        Line::from(health_spans),
        Line::from(usage_spans),
    ];

    let paragraph = Paragraph::new(status_text).style(Style::default().bg(theme::BG_HARD));
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Help overlay
// ---------------------------------------------------------------------------

fn render_help_overlay(f: &mut Frame, area: Rect) {
    let popup_width = 50.min(area.width.saturating_sub(4));
    let popup_height = 20.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect::new(x, y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);

    let help_text = vec![
        Line::from(Span::styled(
            " Keyboard Shortcuts ",
            Style::default()
                .fg(theme::BRIGHT_YELLOW)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  q       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Quit", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  j/k     ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Navigate up/down", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  h/l     ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Collapse/Expand tree node", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  Enter   ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Select / Toggle", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  Tab     ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Switch panel", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+N  ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Switch to tab N", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  p       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Pods view", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  d       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Deployments view", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  s       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Services view", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  n       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Nodes view", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  r       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Force refresh", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  ?       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Toggle this help", Style::default().fg(theme::FG)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Press ? or q to close",
            Style::default().fg(theme::FG4),
        )),
    ];

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BRIGHT_YELLOW))
        .style(Style::default().bg(theme::BG_HARD));

    let paragraph = Paragraph::new(help_text).block(block);
    f.render_widget(paragraph, popup_area);
}
