use crate::dash::app::{ActivePanel, App, ConnectionStatus, NodeType, ResourceView};
use crate::dash::data::HealthStatus;
use crate::dash::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// Main render
// ---------------------------------------------------------------------------

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();

    // Background
    f.render_widget(Block::default().style(Style::default().bg(theme::BG)), size);

    // Top-level layout: tab bar (1) | body | status bar (3)
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar
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
            Constraint::Min(20),    // center
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
    let mut spans: Vec<Span> = vec![
        Span::styled(
            " ScaleX ",
            Style::default()
                .fg(theme::BG_HARD)
                .bg(theme::BRIGHT_ORANGE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().bg(theme::BG)),
    ];

    let tab_spans: Vec<Span> = app
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

    spans.extend(tab_spans);

    let line = Line::from(spans);
    let bar = Paragraph::new(line).style(Style::default().bg(theme::BG));
    f.render_widget(bar, area);
}

// ---------------------------------------------------------------------------
// Sidebar (NERDTree-style)
// ---------------------------------------------------------------------------

fn render_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_panel == ActivePanel::Sidebar;
    let border_color = if is_active {
        theme::BRIGHT_YELLOW
    } else {
        theme::BG3
    };

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
            let is_cursor = vi == app.tree_cursor && is_active;

            // Check if this node is the actively selected context (US-005)
            let is_active_selection = match &node.node_type {
                NodeType::Cluster(name) => {
                    app.selected_cluster.as_ref() == Some(name) && app.selected_namespace.is_none()
                }
                NodeType::Namespace { cluster, namespace } => {
                    app.selected_cluster.as_ref() == Some(cluster)
                        && if namespace == "All Namespaces" {
                            app.selected_namespace.is_none()
                        } else {
                            app.selected_namespace.as_ref() == Some(namespace)
                        }
                }
                _ => false,
            };

            let icon = match (&node.node_type, node.expanded) {
                (NodeType::Root, true) => "▼ ",
                (NodeType::Root, false) => "▶ ",
                (NodeType::Cluster(_), true) => "▼ ",
                (NodeType::Cluster(_), false) => "▶ ",
                (NodeType::Namespace { .. }, _) => "  ",
                (NodeType::InfraHeader, true) => "▼ ",
                (NodeType::InfraHeader, false) => "▶ ",
                (NodeType::InfraItem(_), _) => "  ",
            };

            // Selection marker: fixed-width 2 chars to maintain column alignment
            let marker = if is_active_selection && !is_cursor {
                "● "
            } else {
                "  "
            };

            let indent = "  ".repeat(node.depth);
            let label_color = match &node.node_type {
                NodeType::Root => theme::BRIGHT_ORANGE,
                NodeType::Cluster(_) => theme::BRIGHT_BLUE,
                NodeType::Namespace { .. } => theme::FG,
                NodeType::InfraHeader => theme::BRIGHT_AQUA,
                NodeType::InfraItem(_) => theme::FG3,
            };

            let style = if is_cursor {
                // Cursor: yellow bg highlight
                Style::default()
                    .fg(theme::BG_HARD)
                    .bg(theme::BRIGHT_YELLOW)
                    .add_modifier(Modifier::BOLD)
            } else if is_active_selection {
                // Active selection: bold with bright color, no bg change
                Style::default()
                    .fg(theme::BRIGHT_AQUA)
                    .bg(theme::BG_HARD)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(label_color).bg(theme::BG_HARD)
            };

            // Connection status suffix for cluster nodes
            let conn_suffix = match &node.node_type {
                NodeType::Cluster(name) => match app.cluster_connection_status.get(name) {
                    Some(ConnectionStatus::Discovering) => Some((" [..]", theme::FG4)),
                    Some(ConnectionStatus::Failed(_)) => Some((" [!!]", theme::BRIGHT_RED)),
                    Some(ConnectionStatus::Connected) | None => None,
                },
                _ => None,
            };

            let mut spans = vec![
                Span::styled(indent, Style::default().bg(theme::BG_HARD)),
                Span::styled(
                    marker,
                    Style::default().fg(theme::BRIGHT_AQUA).bg(theme::BG_HARD),
                ),
                Span::styled(icon, style),
                Span::styled(&node.label, style),
            ];
            if let Some((suffix, color)) = conn_suffix {
                spans.push(Span::styled(
                    suffix,
                    Style::default().fg(color).bg(theme::BG_HARD),
                ));
            }

            Line::from(spans)
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
    let border_color = if is_active {
        theme::BRIGHT_YELLOW
    } else {
        theme::BG3
    };

    let ctx_label = match (&app.selected_cluster, &app.selected_namespace) {
        (Some(c), Some(ns)) => format!("{} > {}", c, ns),
        (Some(c), None) => format!("{} > All Namespaces", c),
        _ => "No cluster selected".to_string(),
    };

    let mut title_spans = vec![Span::styled(
        format!(" {} ", app.resource_view.label()),
        Style::default().fg(theme::FG),
    )];
    if app.is_view_stale(app.resource_view) {
        title_spans.push(Span::styled(
            "[cached] ",
            Style::default().fg(theme::BRIGHT_ORANGE),
        ));
    }
    title_spans.push(Span::styled(
        format!("| {} ", ctx_label),
        Style::default().fg(theme::FG),
    ));
    let block = Block::default()
        .title(Line::from(title_spans))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme::BG));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split inner area for search bar if active
    let (content_area, search_area) = if app.search_active {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(1)])
            .split(inner);
        (split[0], Some(split[1]))
    } else {
        (inner, None)
    };

    match app.active_tab {
        0 => render_resources_tab(f, app, content_area),
        1 => render_top_tab(f, app, content_area),
        _ => {
            let msg = Paragraph::new("Custom tab (press ? for help)")
                .style(Style::default().fg(theme::FG4));
            f.render_widget(msg, content_area);
        }
    }

    // Render search bar
    if let Some(area) = search_area {
        let query = app.search_query.as_deref().unwrap_or("");
        let search_line = Line::from(vec![
            Span::styled(
                "/ ",
                Style::default()
                    .fg(theme::BRIGHT_YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(query, Style::default().fg(theme::FG)),
            Span::styled("_", Style::default().fg(theme::BRIGHT_YELLOW)),
        ]);
        let bar = Paragraph::new(search_line).style(Style::default().bg(theme::BG1));
        f.render_widget(bar, area);
    }
}

fn render_resources_tab(f: &mut Frame, app: &App, area: Rect) {
    // Check for connection failure before attempting snapshot lookup
    if let Some(cluster_name) = &app.selected_cluster {
        if let Some(ConnectionStatus::Failed(err_msg)) =
            app.cluster_connection_status.get(cluster_name)
        {
            let lines = vec![
                Line::from(vec![
                    Span::styled("  [!!] ", Style::default().fg(theme::BRIGHT_RED)),
                    Span::styled(
                        format!("Cannot connect to {}", cluster_name),
                        Style::default().fg(theme::BRIGHT_RED),
                    ),
                ]),
                Line::from(Span::styled(
                    format!("       {}", err_msg),
                    Style::default().fg(theme::FG4),
                )),
                Line::from(Span::styled(
                    "       Press 'r' to retry",
                    Style::default().fg(theme::FG4),
                )),
            ];
            let paragraph = Paragraph::new(lines).style(Style::default().bg(theme::BG));
            f.render_widget(paragraph, area);
            return;
        }
    }

    let snapshot = match app.current_snapshot() {
        Some(s) => s,
        None => {
            let spinner_chars = ['|', '/', '-', '\\'];
            let spinner = spinner_chars[(app.tick_count as usize) % 4];
            let msg = if !app.discover_complete {
                format!("  {} Discovering clusters...", spinner)
            } else if app.snapshots.is_empty() && app.is_fetching {
                format!("  {} Loading cluster data...", spinner)
            } else if app.snapshots.is_empty() && app.clusters.is_empty() {
                "  No clusters found. Run 'scalex cluster init' first.".to_string()
            } else if app.selected_cluster.is_some() && app.is_fetching {
                format!("  {} Loading...", spinner)
            } else {
                "  Select a cluster and press Enter".to_string()
            };
            let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
            f.render_widget(paragraph, area);
            return;
        }
    };

    match app.resource_view {
        ResourceView::Pods => render_pods_table(f, app, &snapshot.pods, area),
        ResourceView::Deployments => render_deployments_table(f, app, &snapshot.deployments, area),
        ResourceView::Services => render_services_table(f, app, &snapshot.services, area),
        ResourceView::Nodes => render_nodes_table(f, app, &snapshot.nodes, area),
        ResourceView::ConfigMaps => render_configmaps_table(f, app, &snapshot.configmaps, area),
    }
}

fn render_pods_table(f: &mut Frame, app: &App, pods: &[crate::dash::data::PodInfo], area: Rect) {
    let header = Row::new(vec![
        "NAME",
        "NAMESPACE",
        "STATUS",
        "READY",
        "RESTARTS",
        "AGE",
        "NODE",
    ])
    .style(
        Style::default()
            .fg(theme::BRIGHT_YELLOW)
            .bg(theme::BG1)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(0);

    let filtered: Vec<&crate::dash::data::PodInfo> = pods
        .iter()
        .filter(|p| app.matches_search(&p.name))
        .collect();

    if filtered.is_empty() {
        let msg = if app.search_query.as_ref().is_some_and(|q| !q.is_empty()) {
            format!(
                "  No results for \"{}\"",
                app.search_query.as_deref().unwrap_or("")
            )
        } else {
            "  No pods in this namespace".to_string()
        };
        let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
        f.render_widget(paragraph, area);
        return;
    }

    let rows: Vec<Row> = filtered
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
                Cell::from(pod.status.as_str()).style(if is_selected {
                    Style::default().fg(status_color).bg(theme::BRIGHT_YELLOW)
                } else {
                    Style::default().fg(status_color)
                }),
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
    app: &App,
    deployments: &[crate::dash::data::DeploymentInfo],
    area: Rect,
) {
    let header = Row::new(vec![
        "NAME",
        "NAMESPACE",
        "READY",
        "UP-TO-DATE",
        "AVAILABLE",
        "AGE",
    ])
    .style(
        Style::default()
            .fg(theme::BRIGHT_YELLOW)
            .bg(theme::BG1)
            .add_modifier(Modifier::BOLD),
    );

    let filtered: Vec<&crate::dash::data::DeploymentInfo> = deployments
        .iter()
        .filter(|d| app.matches_search(&d.name))
        .collect();

    if filtered.is_empty() {
        let msg = if app.search_query.as_ref().is_some_and(|q| !q.is_empty()) {
            format!(
                "  No results for \"{}\"",
                app.search_query.as_deref().unwrap_or("")
            )
        } else {
            "  No deployments in this namespace".to_string()
        };
        let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
        f.render_widget(paragraph, area);
        return;
    }

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, dep)| {
            let is_selected = i == app.table_cursor && app.active_panel == ActivePanel::Center;
            let base = if is_selected {
                Style::default().fg(theme::BG_HARD).bg(theme::BRIGHT_YELLOW)
            } else {
                Style::default().fg(theme::FG).bg(theme::BG)
            };
            Row::new(vec![
                Cell::from(dep.name.as_str()).style(base),
                Cell::from(dep.namespace.as_str()).style(base),
                Cell::from(dep.ready.as_str()).style(base),
                Cell::from(dep.up_to_date.to_string()).style(base),
                Cell::from(dep.available.to_string()).style(base),
                Cell::from(dep.age.as_str()).style(base),
            ])
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
    app: &App,
    services: &[crate::dash::data::ServiceInfo],
    area: Rect,
) {
    let header = Row::new(vec![
        "NAME",
        "NAMESPACE",
        "TYPE",
        "CLUSTER-IP",
        "PORTS",
        "AGE",
    ])
    .style(
        Style::default()
            .fg(theme::BRIGHT_YELLOW)
            .bg(theme::BG1)
            .add_modifier(Modifier::BOLD),
    );

    let filtered: Vec<&crate::dash::data::ServiceInfo> = services
        .iter()
        .filter(|s| app.matches_search(&s.name))
        .collect();

    if filtered.is_empty() {
        let msg = if app.search_query.as_ref().is_some_and(|q| !q.is_empty()) {
            format!(
                "  No results for \"{}\"",
                app.search_query.as_deref().unwrap_or("")
            )
        } else {
            "  No services in this namespace".to_string()
        };
        let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
        f.render_widget(paragraph, area);
        return;
    }

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, svc)| {
            let is_selected = i == app.table_cursor && app.active_panel == ActivePanel::Center;
            let base = if is_selected {
                Style::default().fg(theme::BG_HARD).bg(theme::BRIGHT_YELLOW)
            } else {
                Style::default().fg(theme::FG).bg(theme::BG)
            };
            Row::new(vec![
                Cell::from(svc.name.as_str()).style(base),
                Cell::from(svc.namespace.as_str()).style(base),
                Cell::from(svc.svc_type.as_str()).style(base),
                Cell::from(svc.cluster_ip.as_str()).style(base),
                Cell::from(svc.ports.as_str()).style(base),
                Cell::from(svc.age.as_str()).style(base),
            ])
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

fn render_nodes_table(f: &mut Frame, app: &App, nodes: &[crate::dash::data::NodeInfo], area: Rect) {
    let header = Row::new(vec!["NAME", "STATUS", "ROLES", "CPU", "MEMORY"]).style(
        Style::default()
            .fg(theme::BRIGHT_YELLOW)
            .bg(theme::BG1)
            .add_modifier(Modifier::BOLD),
    );

    let filtered: Vec<&crate::dash::data::NodeInfo> = nodes
        .iter()
        .filter(|n| app.matches_search(&n.name))
        .collect();

    if filtered.is_empty() {
        let msg = if app.search_query.as_ref().is_some_and(|q| !q.is_empty()) {
            format!(
                "  No results for \"{}\"",
                app.search_query.as_deref().unwrap_or("")
            )
        } else {
            "  No nodes in this namespace".to_string()
        };
        let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
        f.render_widget(paragraph, area);
        return;
    }

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let is_selected = i == app.table_cursor && app.active_panel == ActivePanel::Center;
            let status_color = if node.status == "Ready" {
                theme::BRIGHT_GREEN
            } else {
                theme::BRIGHT_RED
            };

            if is_selected {
                let base = Style::default().fg(theme::BG_HARD).bg(theme::BRIGHT_YELLOW);
                Row::new(vec![
                    Cell::from(node.name.as_str()).style(base),
                    Cell::from(node.status.as_str())
                        .style(Style::default().fg(status_color).bg(theme::BRIGHT_YELLOW)),
                    Cell::from(node.roles.join(",")).style(base),
                    Cell::from(format!("{}/{}", node.cpu_allocatable, node.cpu_capacity))
                        .style(base),
                    Cell::from(format!("{}/{}", node.mem_allocatable, node.mem_capacity))
                        .style(base),
                ])
            } else {
                Row::new(vec![
                    Cell::from(node.name.as_str()).style(Style::default().fg(theme::FG)),
                    Cell::from(node.status.as_str()).style(Style::default().fg(status_color)),
                    Cell::from(node.roles.join(",")).style(Style::default().fg(theme::FG3)),
                    Cell::from(format!("{}/{}", node.cpu_allocatable, node.cpu_capacity))
                        .style(Style::default().fg(theme::BRIGHT_AQUA)),
                    Cell::from(format!("{}/{}", node.mem_allocatable, node.mem_capacity))
                        .style(Style::default().fg(theme::BRIGHT_PURPLE)),
                ])
            }
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

fn render_configmaps_table(
    f: &mut Frame,
    app: &App,
    configmaps: &[crate::dash::data::ConfigMapInfo],
    area: Rect,
) {
    let header = Row::new(vec!["NAME", "NAMESPACE", "KEYS", "AGE"]).style(
        Style::default()
            .fg(theme::BRIGHT_YELLOW)
            .bg(theme::BG1)
            .add_modifier(Modifier::BOLD),
    );

    let filtered: Vec<&crate::dash::data::ConfigMapInfo> = configmaps
        .iter()
        .filter(|cm| app.matches_search(&cm.name))
        .collect();

    if filtered.is_empty() {
        let msg = if app.search_query.as_ref().is_some_and(|q| !q.is_empty()) {
            format!(
                "  No results for \"{}\"",
                app.search_query.as_deref().unwrap_or("")
            )
        } else {
            "  No configmaps in this namespace".to_string()
        };
        let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
        f.render_widget(paragraph, area);
        return;
    }

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, cm)| {
            let is_selected = i == app.table_cursor && app.active_panel == ActivePanel::Center;
            let base = if is_selected {
                Style::default().fg(theme::BG_HARD).bg(theme::BRIGHT_YELLOW)
            } else {
                Style::default().fg(theme::FG).bg(theme::BG)
            };

            Row::new(vec![
                Cell::from(cm.name.as_str()).style(base),
                Cell::from(cm.namespace.as_str()).style(base),
                Cell::from(cm.data_keys_count.to_string()).style(base),
                Cell::from(cm.age.as_str()).style(base),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(35),
            Constraint::Percentage(25),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
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
            let spinner_chars = ['|', '/', '-', '\\'];
            let spinner = spinner_chars[(app.tick_count as usize) % 4];
            let msg = if !app.discover_complete {
                format!("  {} Discovering clusters...", spinner)
            } else if app.is_fetching {
                format!("  {} Loading...", spinner)
            } else {
                "  Select a cluster and press Enter".to_string()
            };
            let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
            f.render_widget(paragraph, area);
            return;
        }
    };

    let mut lines = vec![
        Line::from(vec![Span::styled(
            " Node Resource Utilization ",
            Style::default()
                .fg(theme::BRIGHT_YELLOW)
                .add_modifier(Modifier::BOLD),
        )]),
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
            Span::styled(
                format!(" {} ", status_icon),
                Style::default().fg(status_color),
            ),
            Span::styled(
                &node.name,
                Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  CPU: {}/{}  MEM: {}/{}",
                    node.cpu_allocatable,
                    node.cpu_capacity,
                    node.mem_allocatable,
                    node.mem_capacity
                ),
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

/// Render a compact utilization bar: `CPU [========--] 82%` or `CPU N/A`
fn render_usage_bar<'a>(label: &'a str, percent: f64, width: usize, color: Color) -> Vec<Span<'a>> {
    if percent <= 0.0 {
        return vec![
            Span::styled(format!("{} ", label), Style::default().fg(theme::FG4)),
            Span::styled("N/A ", Style::default().fg(theme::FG4)),
        ];
    }
    let filled = ((percent / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;
    let bar_color = if percent > 90.0 {
        theme::BRIGHT_RED
    } else if percent > 70.0 {
        theme::BRIGHT_YELLOW
    } else {
        color
    };

    vec![
        Span::styled(format!("{} [", label), Style::default().fg(theme::FG4)),
        Span::styled("=".repeat(filled), Style::default().fg(bar_color)),
        Span::styled("-".repeat(empty), Style::default().fg(theme::FG4)),
        Span::styled(
            format!("] {:>3.0}% ", percent),
            Style::default().fg(theme::FG3),
        ),
    ]
}

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme::BG3))
        .style(Style::default().bg(theme::BG_HARD));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Line 1: cluster health indicators + pod/node counts
    let spinner_chars = ['|', '/', '-', '\\'];
    let spinner = spinner_chars[(app.tick_count as usize) % 4];

    let mut health_spans: Vec<Span> = if !app.discover_complete {
        vec![Span::styled(
            format!(" {} Discovering clusters...", spinner),
            Style::default().fg(theme::BRIGHT_YELLOW),
        )]
    } else if app.snapshots.is_empty() && app.is_fetching {
        vec![Span::styled(
            format!(" {} Loading cluster data...", spinner),
            Style::default().fg(theme::BRIGHT_YELLOW),
        )]
    } else if app.snapshots.is_empty() {
        vec![Span::styled(
            " Waiting for cluster data...",
            Style::default().fg(theme::BRIGHT_YELLOW),
        )]
    } else {
        vec![Span::styled(" Clusters: ", Style::default().fg(theme::FG4))]
    };

    for snapshot in &app.snapshots {
        let (symbol, color) = match snapshot.health {
            HealthStatus::Green => ("●", theme::BRIGHT_GREEN),
            HealthStatus::Yellow => ("●", theme::BRIGHT_YELLOW),
            HealthStatus::Red => ("●", theme::BRIGHT_RED),
            HealthStatus::Unknown => ("○", theme::FG4),
        };
        let ru = &snapshot.resource_usage;
        health_spans.push(Span::styled(
            format!("{} ", symbol),
            Style::default().fg(color),
        ));
        health_spans.push(Span::styled(
            format!(
                "{} pods:{}/{} nodes:{}/{}  ",
                snapshot.name, ru.running_pods, ru.total_pods, ru.ready_nodes, ru.total_nodes
            ),
            Style::default().fg(theme::FG3),
        ));
    }

    // Line 2: CPU/Mem bars per cluster + self overhead + latency
    let mut usage_spans: Vec<Span> = vec![Span::styled(" ", Style::default().fg(theme::FG4))];

    for snapshot in &app.snapshots {
        let ru = &snapshot.resource_usage;
        usage_spans.push(Span::styled(
            format!("{}: ", snapshot.name),
            Style::default().fg(theme::FG3),
        ));
        usage_spans.extend(render_usage_bar(
            "CPU",
            ru.cpu_percent,
            8,
            theme::BRIGHT_AQUA,
        ));
        usage_spans.extend(render_usage_bar(
            "MEM",
            ru.mem_percent,
            8,
            theme::BRIGHT_PURPLE,
        ));
    }

    // Self overhead + fetch indicator
    let rss_str = app
        .self_rss_mb
        .map(|mb| format!("{:.0}MB", mb))
        .unwrap_or_else(|| "N/A".into());
    let fetch_indicator = if app.is_fetching {
        format!(" {} ", spinner)
    } else {
        String::new()
    };
    usage_spans.push(Span::styled(
        format!(
            "| self: {} | latency: {}ms{}",
            rss_str, app.api_latency_ms, fetch_indicator
        ),
        Style::default().fg(theme::BRIGHT_AQUA),
    ));

    let status_text = vec![Line::from(health_spans), Line::from(usage_spans)];

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
            Span::styled("Move cursor (no selection)", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  h/l     ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Collapse/Expand tree node", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  Enter   ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Select cluster/namespace", Style::default().fg(theme::FG)),
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
            Span::styled("  c       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("ConfigMaps view", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  n       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Nodes view", Style::default().fg(theme::FG)),
        ]),
        Line::from(vec![
            Span::styled("  /       ", Style::default().fg(theme::BRIGHT_AQUA)),
            Span::styled("Search (filter by name)", Style::default().fg(theme::FG)),
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
