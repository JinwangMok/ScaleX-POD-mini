use crate::dash::app::{ActivePanel, App, ConnectionStatus, NodeType, ResourceView};
use crate::dash::data::{self, HealthStatus};
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

    // Top-level layout: header (responsive) | body | status bar (3)
    let header_height = if size.height >= 28 { 8 } else { 4 };
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(5),    // body
            Constraint::Length(3), // status bar
        ])
        .split(size);

    render_header(f, app, vertical[0]);

    // Body: sidebar | center (responsive sidebar width)
    let sidebar_width = if size.width < 60 {
        20
    } else if size.width < 80 {
        24
    } else {
        28
    };
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(sidebar_width),
            Constraint::Min(20), // center
        ])
        .split(vertical[1]);

    render_sidebar(f, app, horizontal[0]);
    render_center(f, app, horizontal[1]);
    render_status_bar(f, app, vertical[2]);

    if app.show_help {
        render_help_overlay(f, app, size);
    }
}

// ---------------------------------------------------------------------------
// Header (k9s-style dashboard header)
// ---------------------------------------------------------------------------

/// ASCII art logo for full-size header (6 lines)
const LOGO: [&str; 6] = [
    r"███████╗ ██████╗ █████╗ ██╗     ███████╗██╗  ██╗",
    r"██╔════╝██╔════╝██╔══██╗██║     ██╔════╝╚██╗██╔╝",
    r"███████╗██║     ███████║██║     █████╗   ╚███╔╝ ",
    r"╚════██║██║     ██╔══██║██║     ██╔══╝   ██╔██╗ ",
    r"███████║╚██████╗██║  ██║███████╗███████╗██╔╝ ██╗",
    r"╚══════╝ ╚═════╝╚═╝  ╚═╝╚══════╝╚══════╝╚═╝  ╚═╝",
];

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let is_full = area.height >= 8;

    // Gather cluster info for the selected (or first) cluster
    let selected = app
        .selected_cluster
        .as_ref()
        .and_then(|name| app.clusters.iter().find(|c| &c.name == name))
        .or_else(|| app.clusters.first());

    let cluster_name = selected.map(|c| c.name.as_str()).unwrap_or("--");
    let endpoint_str = selected.and_then(|c| c.endpoint.as_deref()).unwrap_or("--");
    let k8s_ver = selected
        .and_then(|c| c.server_version.as_deref())
        .unwrap_or("N/A");
    let config_path = selected
        .map(|c| c.kubeconfig_path.display().to_string())
        .unwrap_or_else(|| "--".into());

    let scalex_ver = env!("CARGO_PKG_VERSION");

    let total_clusters = app.cluster_connection_status.len();
    let connected_clusters = app
        .cluster_connection_status
        .values()
        .filter(|s| matches!(s, ConnectionStatus::Connected))
        .count();

    let label_style = Style::default().fg(theme::FG4);
    let value_style = Style::default().fg(theme::FG);
    let accent_style = Style::default().fg(theme::BRIGHT_AQUA);

    // Tab spans (shared between both modes)
    let tab_spans = build_tab_spans(app);

    if is_full {
        render_header_full(
            f,
            area,
            &tab_spans,
            cluster_name,
            endpoint_str,
            k8s_ver,
            &config_path,
            scalex_ver,
            total_clusters,
            connected_clusters,
            label_style,
            value_style,
            accent_style,
        );
    } else {
        render_header_compact(
            f,
            area,
            &tab_spans,
            cluster_name,
            endpoint_str,
            k8s_ver,
            scalex_ver,
            total_clusters,
            connected_clusters,
            label_style,
            value_style,
            accent_style,
        );
    }
}

fn build_tab_spans(app: &App) -> Vec<Span<'static>> {
    app.tabs
        .iter()
        .enumerate()
        .flat_map(|(i, tab)| {
            let style = if i == app.active_tab {
                Style::default()
                    .fg(theme::BG_HARD)
                    .bg(theme::BRIGHT_YELLOW)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::FG4).bg(theme::BG1)
            };
            vec![
                Span::styled(format!(" [{}] ", i + 1), style),
                Span::styled(tab.name.clone(), style),
                Span::styled(" ", Style::default().bg(theme::BG_HARD)),
            ]
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render_header_full(
    f: &mut Frame,
    area: Rect,
    tab_spans: &[Span<'static>],
    cluster_name: &str,
    endpoint_str: &str,
    k8s_ver: &str,
    config_path: &str,
    scalex_ver: &str,
    total_clusters: usize,
    connected_clusters: usize,
    label_style: Style,
    value_style: Style,
    accent_style: Style,
) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme::BG3))
        .style(Style::default().bg(theme::BG_HARD));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split: left info | right logo
    let logo_width: u16 = 52; // widest LOGO line
    let show_logo = inner.width > logo_width + 30;

    let cols = if show_logo {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(30), Constraint::Length(logo_width + 1)])
            .split(inner)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100)])
            .split(inner)
    };

    // Left: info lines
    let info_lines = vec![
        Line::from(vec![
            Span::styled(" Context:  ", label_style),
            Span::styled(cluster_name.to_string(), accent_style),
        ]),
        Line::from(vec![
            Span::styled(" Cluster:  ", label_style),
            Span::styled(endpoint_str.to_string(), value_style),
        ]),
        Line::from(vec![
            Span::styled(" ScaleX:   ", label_style),
            Span::styled(
                format!("v{}", scalex_ver),
                Style::default()
                    .fg(theme::BRIGHT_ORANGE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("    Clusters: {}/{}", connected_clusters, total_clusters),
                Style::default().fg(theme::FG3),
            ),
        ]),
        Line::from(vec![
            Span::styled(" K8s Rev:  ", label_style),
            Span::styled(
                k8s_ver.to_string(),
                if k8s_ver == "N/A" {
                    label_style
                } else {
                    value_style
                },
            ),
        ]),
        Line::from(vec![
            Span::styled(" Config:   ", label_style),
            Span::styled(config_path.to_string(), Style::default().fg(theme::FG3)),
        ]),
        Line::from(
            vec![Span::styled(" View:     ", label_style)]
                .into_iter()
                .chain(tab_spans.iter().cloned())
                .collect::<Vec<_>>(),
        ),
    ];

    let para = Paragraph::new(info_lines).style(Style::default().bg(theme::BG_HARD));
    f.render_widget(para, cols[0]);

    // Right: ASCII art logo
    if show_logo && cols.len() > 1 {
        let logo_lines: Vec<Line> = LOGO
            .iter()
            .map(|line| {
                Line::from(Span::styled(
                    *line,
                    Style::default()
                        .fg(theme::BRIGHT_ORANGE)
                        .add_modifier(Modifier::BOLD),
                ))
            })
            .collect();
        let logo_para = Paragraph::new(logo_lines).style(Style::default().bg(theme::BG_HARD));
        f.render_widget(logo_para, cols[1]);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_header_compact(
    f: &mut Frame,
    area: Rect,
    tab_spans: &[Span<'static>],
    cluster_name: &str,
    endpoint_str: &str,
    k8s_ver: &str,
    scalex_ver: &str,
    total_clusters: usize,
    connected_clusters: usize,
    label_style: Style,
    _value_style: Style,
    accent_style: Style,
) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme::BG3))
        .style(Style::default().bg(theme::BG_HARD));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let line1_spans = vec![
        Span::styled(
            " ScaleX ",
            Style::default()
                .fg(theme::BG_HARD)
                .bg(theme::BRIGHT_ORANGE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" v{}  ", scalex_ver),
            Style::default().fg(theme::BRIGHT_ORANGE),
        ),
        Span::styled(cluster_name.to_string(), accent_style),
        Span::styled(
            format!(
                "  Clusters: {}/{}  K8s: {}",
                connected_clusters, total_clusters, k8s_ver
            ),
            Style::default().fg(theme::FG3),
        ),
    ];

    let mut line2_spans: Vec<Span> = vec![Span::styled(" ", label_style)];
    line2_spans.extend(tab_spans.iter().cloned());
    line2_spans.push(Span::styled(
        format!("  {}", endpoint_str),
        Style::default().fg(theme::FG4),
    ));

    let lines = vec![Line::from(line1_spans), Line::from(line2_spans)];
    let para = Paragraph::new(lines).style(Style::default().bg(theme::BG_HARD));
    f.render_widget(para, inner);
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
                    app.selected_cluster.as_ref() == Some(name)
                        && app.selected_namespace.is_none()
                        && !node.expanded // collapsed cluster = active; expanded = children handle it
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
            let marker = if is_active_selection { "● " } else { "  " };

            let indent = "  ".repeat(node.depth);
            let label_color = match &node.node_type {
                NodeType::Root => theme::BRIGHT_ORANGE,
                NodeType::Cluster(_) => theme::BRIGHT_BLUE,
                NodeType::Namespace { .. } => theme::FG,
                NodeType::InfraHeader => theme::BRIGHT_AQUA,
                NodeType::InfraItem(_) => theme::FG3,
            };

            let (style, marker_style, suffix_bg) = if is_cursor {
                // Cursor: full-width yellow bg highlight
                let cursor_style = Style::default()
                    .fg(theme::BG_HARD)
                    .bg(theme::BRIGHT_YELLOW)
                    .add_modifier(Modifier::BOLD);
                (cursor_style, cursor_style, theme::BRIGHT_YELLOW)
            } else if is_active_selection {
                // Active selection: bold with bright color, no bg change
                let sel_style = Style::default()
                    .fg(theme::BRIGHT_AQUA)
                    .bg(theme::BG_HARD)
                    .add_modifier(Modifier::BOLD);
                let marker_s = Style::default().fg(theme::BRIGHT_AQUA).bg(theme::BG_HARD);
                (sel_style, marker_s, theme::BG_HARD)
            } else {
                let normal = Style::default().fg(label_color).bg(theme::BG_HARD);
                let marker_s = Style::default().bg(theme::BG_HARD);
                (normal, marker_s, theme::BG_HARD)
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

            // Truncate label to fit sidebar width
            let prefix_width = indent.len() + marker.len() + icon.len();
            let suffix_width = conn_suffix.as_ref().map(|(s, _)| s.len()).unwrap_or(0);
            let available = (inner.width as usize).saturating_sub(prefix_width + suffix_width);
            let display_label: String = if node.label.len() > available && available > 1 {
                let truncated: String = node.label.chars().take(available - 1).collect();
                format!("{}…", truncated)
            } else {
                node.label.clone()
            };

            let label_len = display_label.len();
            let mut spans = vec![
                Span::styled(indent, style),
                Span::styled(marker, marker_style),
                Span::styled(icon, style),
                Span::styled(display_label, style),
            ];
            let mut used_width = prefix_width + label_len;
            if let Some((suffix, color)) = conn_suffix {
                used_width += suffix.len();
                spans.push(Span::styled(
                    suffix,
                    Style::default().fg(color).bg(suffix_bg),
                ));
            }
            // Pad to full sidebar width so cursor/selection highlight fills the row
            let pad = (inner.width as usize).saturating_sub(used_width);
            if pad > 0 {
                let pad_style = if is_cursor {
                    style
                } else {
                    Style::default().bg(theme::BG_HARD)
                };
                spans.push(Span::styled(" ".repeat(pad), pad_style));
            }

            Line::from(spans)
        })
        .collect();

    let paragraph = Paragraph::new(lines).scroll((app.sidebar_scroll_offset as u16, 0));
    f.render_widget(paragraph, inner);

    // Scroll indicator when content overflows viewport
    let visible_len = app.visible_tree_len();
    if visible_len > inner.height as usize {
        let indicator = format!(
            " {}/{} ",
            (app.tree_cursor + 1).min(visible_len),
            visible_len,
        );
        let x = inner.x;
        let y = inner.y + inner.height.saturating_sub(1);
        if y < inner.y + inner.height {
            let indicator_area = Rect::new(x, y, indicator.len().min(inner.width as usize) as u16, 1);
            let indicator_widget = Paragraph::new(Span::styled(
                indicator,
                Style::default().fg(theme::FG4).bg(theme::BG_HARD),
            ));
            f.render_widget(indicator_widget, indicator_area);
        }
    }
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

    // Resource shortcut indicator: p d s c n with active one highlighted
    let resource_shortcuts = [
        ('p', "Pods", ResourceView::Pods),
        ('d', "Deploy", ResourceView::Deployments),
        ('s', "Svc", ResourceView::Services),
        ('c', "CM", ResourceView::ConfigMaps),
        ('n', "Nodes", ResourceView::Nodes),
    ];
    let mut title_spans: Vec<Span> = vec![Span::styled(" ", Style::default().fg(theme::FG))];
    if app.active_tab == 0 {
        for (key, label, view) in &resource_shortcuts {
            if *view == app.resource_view {
                title_spans.push(Span::styled(
                    format!("[{}]{} ", key, label),
                    Style::default()
                        .fg(theme::BG_HARD)
                        .bg(theme::BRIGHT_AQUA)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                title_spans.push(Span::styled(
                    format!("{}:{} ", key, label),
                    Style::default().fg(theme::FG4),
                ));
            }
        }
    } else {
        title_spans.push(Span::styled(
            "Top ",
            Style::default()
                .fg(theme::BRIGHT_YELLOW)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if app.is_view_stale(app.resource_view) {
        title_spans.push(Span::styled(
            "[cached] ",
            Style::default().fg(theme::BRIGHT_ORANGE),
        ));
    }
    // Row count indicator
    if app.active_tab == 0 {
        let row_count = app.current_row_count();
        if row_count > 0 {
            let pos = app.table_cursor.min(row_count.saturating_sub(1)) + 1;
            title_spans.push(Span::styled(
                format!("{}/{} ", pos, row_count),
                Style::default().fg(theme::FG3),
            ));
        }
    }
    // Show active search filter indicator (after search submitted with Enter)
    if !app.search_active {
        if let Some(q) = &app.search_query {
            if !q.is_empty() {
                title_spans.push(Span::styled(
                    format!("[/{}] ", q),
                    Style::default().fg(theme::BRIGHT_AQUA),
                ));
            }
        }
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

/// Render connection error or empty/loading state for a tab.
/// Returns `Some(snapshot)` if data is available, `None` if a placeholder was rendered.
fn render_tab_preamble<'a>(f: &mut Frame, app: &'a App, area: Rect) -> Option<&'a data::ClusterSnapshot> {
    // Check for connection failure
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
            return None;
        }
    }

    if let Some(s) = app.current_snapshot() {
        return Some(s);
    }

    let spinner_chars = ['|', '/', '-', '\\'];
    let spinner = spinner_chars[(app.tick_count as usize) % 4];
    let msg = if !app.discover_complete {
        format!("  {} Discovering clusters...", spinner)
    } else if app.snapshots.is_empty() && app.is_fetching {
        format!("  {} Loading cluster data...", spinner)
    } else if app.all_clusters_failed() {
        "  All clusters failed to connect. Check sidebar for details. Press 'r' to retry.".to_string()
    } else if app.snapshots.is_empty() && app.clusters.is_empty() {
        "  No clusters found. Run 'scalex cluster init' first.".to_string()
    } else if app.is_fetching {
        format!("  {} Loading...", spinner)
    } else {
        "  Select a cluster and press Enter".to_string()
    };
    let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
    f.render_widget(paragraph, area);
    None
}

fn render_resources_tab(f: &mut Frame, app: &App, area: Rect) {
    let snapshot = match render_tab_preamble(f, app, area) {
        Some(s) => s,
        None => return,
    };

    match app.resource_view {
        ResourceView::Pods => render_pods_table(f, app, &snapshot.pods, area),
        ResourceView::Deployments => render_deployments_table(f, app, &snapshot.deployments, area),
        ResourceView::Services => render_services_table(f, app, &snapshot.services, area),
        ResourceView::Nodes => render_nodes_table(f, app, &snapshot.nodes, area),
        ResourceView::ConfigMaps => render_configmaps_table(f, app, &snapshot.configmaps, area),
    }
}

/// Generic table renderer — handles filter, empty state, cursor clamping, viewport, selection highlight.
/// `filter_fn` returns true if the item matches the current search query.
/// `row_fn(index, item, is_selected)` builds a styled Row for each visible item.
#[allow(clippy::too_many_arguments)]
fn render_resource_table<'a, T, F, R>(
    f: &mut Frame,
    app: &App,
    items: &'a [T],
    area: Rect,
    header: Row<'static>,
    widths: &[Constraint],
    empty_msg: &str,
    filter_fn: F,
    row_fn: R,
) where
    F: Fn(&'a T) -> bool,
    R: Fn(usize, &'a T, bool) -> Row<'a>,
{
    let filtered: Vec<&T> = items.iter().filter(|item| filter_fn(item)).collect();

    if filtered.is_empty() {
        let msg = if app.search_query.as_ref().is_some_and(|q| !q.is_empty()) {
            format!(
                "  No results for \"{}\"",
                app.search_query.as_deref().unwrap_or("")
            )
        } else {
            format!("  {}", empty_msg)
        };
        let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
        f.render_widget(paragraph, area);
        return;
    }

    let clamped_cursor = app.table_cursor.min(filtered.len() - 1);
    let viewport_rows = area.height.saturating_sub(1) as usize;
    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .skip(app.table_scroll_offset)
        .take(viewport_rows)
        .map(|(i, item)| {
            let is_selected = i == clamped_cursor && app.active_panel == ActivePanel::Center;
            row_fn(i, item, is_selected)
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .style(Style::default().bg(theme::BG));
    f.render_widget(table, area);
}

/// Standard base style for table rows (selected vs normal)
fn row_base_style(is_selected: bool) -> Style {
    if is_selected {
        Style::default().fg(theme::BG_HARD).bg(theme::BRIGHT_YELLOW)
    } else {
        Style::default().fg(theme::FG).bg(theme::BG)
    }
}

/// Standard header style for resource tables
fn resource_header(columns: Vec<&'static str>) -> Row<'static> {
    Row::new(columns).style(
        Style::default()
            .fg(theme::BRIGHT_YELLOW)
            .bg(theme::BG1)
            .add_modifier(Modifier::BOLD),
    )
}

fn render_pods_table(f: &mut Frame, app: &App, pods: &[crate::dash::data::PodInfo], area: Rect) {
    render_resource_table(
        f,
        app,
        pods,
        area,
        resource_header(vec!["NAME", "NAMESPACE", "STATUS", "READY", "RESTARTS", "AGE", "NODE"]),
        &[
            Constraint::Min(16),
            Constraint::Min(10),
            Constraint::Min(10),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Min(12),
        ],
        "No pods in this namespace",
        |pod| app.matches_search_with_ns(&pod.name, &pod.namespace),
        |_i, pod, is_selected| {
            let base = row_base_style(is_selected);
            let status_color = match pod.status.as_str() {
                "Running" => theme::BRIGHT_GREEN,
                "Pending" | "ContainerCreating" | "PodInitializing" | "Terminating" => {
                    theme::BRIGHT_YELLOW
                }
                "Succeeded" | "Completed" => theme::BRIGHT_BLUE,
                "Failed" | "CrashLoopBackOff" | "Error" | "OOMKilled"
                | "ImagePullBackOff" | "ErrImagePull" | "CreateContainerConfigError"
                | "InvalidImageName" => theme::BRIGHT_RED,
                s if s.starts_with("Init:") => {
                    if s.contains("Error") || s.contains("CrashLoopBackOff") {
                        theme::BRIGHT_RED
                    } else {
                        theme::BRIGHT_YELLOW
                    }
                }
                _ => theme::FG3,
            };
            // Color-code restart count
            let restart_style = if is_selected {
                base
            } else if pod.restarts > 10 {
                Style::default().fg(theme::BRIGHT_RED)
            } else if pod.restarts > 0 {
                Style::default().fg(theme::BRIGHT_YELLOW)
            } else {
                base
            };
            Row::new(vec![
                Cell::from(pod.name.as_str()).style(base),
                Cell::from(pod.namespace.as_str()).style(base),
                Cell::from(pod.status.as_str()).style(if is_selected {
                    base
                } else {
                    Style::default().fg(status_color)
                }),
                Cell::from(pod.ready.as_str()).style(base),
                Cell::from(pod.restarts.to_string()).style(restart_style),
                Cell::from(pod.age.as_str()).style(base),
                Cell::from(pod.node.as_str()).style(base),
            ])
        },
    );
}

fn render_deployments_table(
    f: &mut Frame,
    app: &App,
    deployments: &[crate::dash::data::DeploymentInfo],
    area: Rect,
) {
    render_resource_table(
        f,
        app,
        deployments,
        area,
        resource_header(vec!["NAME", "NAMESPACE", "READY", "UP-TO-DATE", "AVAILABLE", "AGE"]),
        &[
            Constraint::Min(16),
            Constraint::Min(10),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(11),
            Constraint::Length(6),
        ],
        "No deployments in this namespace",
        |dep| app.matches_search_with_ns(&dep.name, &dep.namespace),
        |_i, dep, is_selected| {
            let base = row_base_style(is_selected);
            // Color-code READY column based on readiness ratio
            let ready_style = if is_selected {
                base
            } else {
                // Parse "ready/desired" from dep.ready (e.g., "1/3")
                let parts: Vec<&str> = dep.ready.split('/').collect();
                let (ready, desired) = if parts.len() == 2 {
                    (
                        parts[0].trim().parse::<i32>().unwrap_or(0),
                        parts[1].trim().parse::<i32>().unwrap_or(0),
                    )
                } else {
                    (0, 0)
                };
                let color = if desired == 0 || ready >= desired {
                    theme::BRIGHT_GREEN
                } else if ready > 0 {
                    theme::BRIGHT_YELLOW
                } else {
                    theme::BRIGHT_RED
                };
                Style::default().fg(color)
            };
            Row::new(vec![
                Cell::from(dep.name.as_str()).style(base),
                Cell::from(dep.namespace.as_str()).style(base),
                Cell::from(dep.ready.as_str()).style(ready_style),
                Cell::from(dep.up_to_date.to_string()).style(base),
                Cell::from(dep.available.to_string()).style(base),
                Cell::from(dep.age.as_str()).style(base),
            ])
        },
    );
}

fn render_services_table(
    f: &mut Frame,
    app: &App,
    services: &[crate::dash::data::ServiceInfo],
    area: Rect,
) {
    render_resource_table(
        f,
        app,
        services,
        area,
        resource_header(vec!["NAME", "NAMESPACE", "TYPE", "CLUSTER-IP", "PORTS", "AGE"]),
        &[
            Constraint::Min(16),
            Constraint::Min(10),
            Constraint::Length(12),
            Constraint::Length(16),
            Constraint::Min(12),
            Constraint::Length(6),
        ],
        "No services in this namespace",
        |svc| app.matches_search_with_ns(&svc.name, &svc.namespace),
        |_i, svc, is_selected| {
            let base = row_base_style(is_selected);
            Row::new(vec![
                Cell::from(svc.name.as_str()).style(base),
                Cell::from(svc.namespace.as_str()).style(base),
                Cell::from(svc.svc_type.as_str()).style(base),
                Cell::from(svc.cluster_ip.as_str()).style(base),
                Cell::from(svc.ports.as_str()).style(base),
                Cell::from(svc.age.as_str()).style(base),
            ])
        },
    );
}

fn render_nodes_table(f: &mut Frame, app: &App, nodes: &[crate::dash::data::NodeInfo], area: Rect) {
    render_resource_table(
        f,
        app,
        nodes,
        area,
        resource_header(vec!["NAME", "STATUS", "ROLES", "CPU", "MEMORY"]),
        &[
            Constraint::Min(16),
            Constraint::Length(10),
            Constraint::Min(12),
            Constraint::Min(10),
            Constraint::Min(14),
        ],
        "No nodes found",
        |node| app.matches_search(&node.name),
        |_i, node, is_selected| {
            let base = row_base_style(is_selected);
            let roles_str = if node.roles.is_empty() {
                "<none>".to_string()
            } else {
                node.roles.join(",")
            };
            if is_selected {
                Row::new(vec![
                    Cell::from(node.name.as_str()).style(base),
                    Cell::from(node.status.as_str()).style(base),
                    Cell::from(roles_str).style(base),
                    Cell::from(format!("{}/{}", node.cpu_allocatable, node.cpu_capacity))
                        .style(base),
                    Cell::from(format!("{}/{}", node.mem_allocatable, node.mem_capacity))
                        .style(base),
                ])
            } else {
                let status_color = if node.status == "Ready" {
                    theme::BRIGHT_GREEN
                } else {
                    theme::BRIGHT_RED
                };
                Row::new(vec![
                    Cell::from(node.name.as_str()).style(Style::default().fg(theme::FG)),
                    Cell::from(node.status.as_str()).style(Style::default().fg(status_color)),
                    Cell::from(roles_str).style(Style::default().fg(theme::FG3)),
                    Cell::from(format!("{}/{}", node.cpu_allocatable, node.cpu_capacity))
                        .style(Style::default().fg(theme::BRIGHT_AQUA)),
                    Cell::from(format!("{}/{}", node.mem_allocatable, node.mem_capacity))
                        .style(Style::default().fg(theme::BRIGHT_PURPLE)),
                ])
            }
        },
    );
}

fn render_configmaps_table(
    f: &mut Frame,
    app: &App,
    configmaps: &[crate::dash::data::ConfigMapInfo],
    area: Rect,
) {
    render_resource_table(
        f,
        app,
        configmaps,
        area,
        resource_header(vec!["NAME", "NAMESPACE", "KEYS", "AGE"]),
        &[
            Constraint::Min(20),
            Constraint::Min(14),
            Constraint::Length(8),
            Constraint::Length(6),
        ],
        "No configmaps in this namespace",
        |cm| app.matches_search_with_ns(&cm.name, &cm.namespace),
        |_i, cm, is_selected| {
            let base = row_base_style(is_selected);
            Row::new(vec![
                Cell::from(cm.name.as_str()).style(base),
                Cell::from(cm.namespace.as_str()).style(base),
                Cell::from(cm.data_keys_count.to_string()).style(base),
                Cell::from(cm.age.as_str()).style(base),
            ])
        },
    );
}

fn render_top_tab(f: &mut Frame, app: &App, area: Rect) {
    let snapshot = match render_tab_preamble(f, app, area) {
        Some(s) => s,
        None => return,
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

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(theme::BG))
        .scroll((app.table_scroll_offset as u16, 0));
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
    } else if app.all_clusters_failed() {
        vec![Span::styled(
            " [!!] All clusters failed — press 'r' to retry",
            Style::default().fg(theme::BRIGHT_RED),
        )]
    } else if app.snapshots.is_empty() {
        vec![Span::styled(
            " Waiting for cluster data...",
            Style::default().fg(theme::BRIGHT_YELLOW),
        )]
    } else {
        vec![Span::styled(" Clusters: ", Style::default().fg(theme::FG4))]
    };

    let narrow = inner.width < 100;
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
        if narrow {
            // Abbreviated: name + pod count only
            health_spans.push(Span::styled(
                format!("{} {}/{}  ", snapshot.name, ru.running_pods, ru.total_pods),
                Style::default().fg(theme::FG3),
            ));
        } else {
            health_spans.push(Span::styled(
                format!(
                    "{} pods:{}/{} nodes:{}/{}  ",
                    snapshot.name, ru.running_pods, ru.total_pods, ru.ready_nodes, ru.total_nodes
                ),
                Style::default().fg(theme::FG3),
            ));
        }
    }

    // Line 2: CPU/Mem bars per cluster + self overhead + latency
    let mut usage_spans: Vec<Span> = vec![Span::styled(" ", Style::default().fg(theme::FG4))];

    let bar_width = if narrow { 5 } else { 8 };
    for snapshot in &app.snapshots {
        let ru = &snapshot.resource_usage;
        usage_spans.push(Span::styled(
            format!("{}: ", snapshot.name),
            Style::default().fg(theme::FG3),
        ));
        usage_spans.extend(render_usage_bar(
            "CPU",
            ru.cpu_percent,
            bar_width,
            theme::BRIGHT_AQUA,
        ));
        usage_spans.extend(render_usage_bar(
            "MEM",
            ru.mem_percent,
            bar_width,
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
    if app.fetch_timed_out {
        usage_spans.push(Span::styled(
            " [!] fetch timed out — press 'r' to retry",
            Style::default().fg(theme::BRIGHT_RED),
        ));
    }

    let status_text = vec![Line::from(health_spans), Line::from(usage_spans)];

    let paragraph = Paragraph::new(status_text).style(Style::default().bg(theme::BG_HARD));
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Help overlay
// ---------------------------------------------------------------------------

/// Render context-sensitive help overlay.
/// Reads: app.active_panel, app.active_tab, app.resource_view, app.search_active
fn render_help_overlay(f: &mut Frame, app: &App, area: Rect) {
    // -- Determine context title --
    let context_label = if app.search_active {
        "Search".to_string()
    } else {
        match app.active_panel {
            ActivePanel::Sidebar => "Sidebar".to_string(),
            ActivePanel::Center => {
                if app.active_tab == 1 {
                    "Top".to_string()
                } else {
                    app.resource_view.label().to_string()
                }
            }
        }
    };
    let title = format!(" Help — {} ", context_label);

    // -- Build help lines --
    let key = |k: &str, desc: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!("  {:<10}", k),
                Style::default().fg(theme::BRIGHT_AQUA),
            ),
            Span::styled(desc.to_string(), Style::default().fg(theme::FG)),
        ])
    };
    let section = |label: &str| -> Line<'static> {
        Line::from(Span::styled(
            format!(" {} ", label),
            Style::default()
                .fg(theme::BRIGHT_YELLOW)
                .add_modifier(Modifier::BOLD),
        ))
    };

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Context-specific section
    if app.search_active {
        lines.push(section("Search Mode"));
        lines.push(Line::from(""));
        lines.push(key("<type>", "Filter by name/namespace"));
        lines.push(key("Enter", "Confirm search"));
        lines.push(key("ESC", "Cancel search"));
        lines.push(key("Backspace", "Delete character"));
    } else {
        match app.active_panel {
            ActivePanel::Sidebar => {
                lines.push(section("Sidebar Navigation"));
                lines.push(Line::from(""));
                lines.push(key("j/k", "Move cursor (no selection)"));
                lines.push(key("PgUp/Dn", "Jump half page"));
                lines.push(key("Home/End", "Jump to first/last"));
                lines.push(key("h/l", "Collapse/Expand; Left on leaf → parent"));
                lines.push(key("Enter", "Select cluster/namespace"));
            }
            ActivePanel::Center => {
                if app.active_tab == 1 {
                    lines.push(section("Top — Node Resources"));
                    lines.push(Line::from(""));
                    lines.push(key("j/k", "Scroll nodes"));
                    lines.push(key("PgUp/Dn", "Jump half page"));
                    lines.push(key("Home/End", "Jump to first/last"));
                } else {
                    let view = app.resource_view.label();
                    lines.push(section(&format!("Resources — {}", view)));
                    lines.push(Line::from(""));
                    lines.push(key("j/k", "Scroll table rows"));
                    lines.push(key("PgUp/Dn", "Jump half page"));
                    lines.push(key("Home/End", "Jump to first/last"));
                    lines.push(key("p d s c n", "Switch resource view"));
                    lines.push(Line::from(vec![
                        Span::styled("            ".to_string(), Style::default().fg(theme::FG4)),
                        Span::styled(
                            "p=Pods d=Deploy s=Svc c=CM n=Nodes".to_string(),
                            Style::default().fg(theme::FG4),
                        ),
                    ]));
                }
            }
        }
    }

    // Global section
    lines.push(Line::from(""));
    lines.push(section("Global"));
    lines.push(Line::from(""));
    lines.push(key("q/Ctrl+C", "Quit"));
    lines.push(key("Tab", "Switch panel (Sidebar ↔ Center)"));
    lines.push(key("Shift+Tab", "Switch panel (reverse)"));
    lines.push(key("Ctrl+N", "Switch to tab N"));
    lines.push(key("/", "Search (filter by name/namespace)"));
    lines.push(key("ESC", "Clear active filter / close overlay"));
    lines.push(key("r", "Force refresh"));
    lines.push(key("?", "Toggle this help"));

    // Footer
    lines.push(Line::from(""));
    let footer_text = if app.search_query.as_ref().is_some_and(|q| !q.is_empty()) && !app.search_active {
        "  Press ESC to clear filter, ? to close"
    } else {
        "  Press ESC or ? to close"
    };
    lines.push(Line::from(Span::styled(
        footer_text.to_string(),
        Style::default().fg(theme::FG4),
    )));

    // -- Layout: auto-size height, centered --
    let popup_width = 50.min(area.width.saturating_sub(4));
    let content_height = lines.len() as u16;
    let max_popup_height = area.height.saturating_sub(2).max(5); // leave margin, min 5
    let popup_height = (content_height + 2).min(max_popup_height); // +2 for borders
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect::new(x, y, popup_width, popup_height);
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BRIGHT_YELLOW))
        .style(Style::default().bg(theme::BG_HARD));

    // Clamp user scroll offset to valid range
    let inner_height = popup_height.saturating_sub(2);
    let max_scroll = content_height.saturating_sub(inner_height);
    let scroll_offset = app.help_scroll_offset.min(max_scroll);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll_offset, 0));
    f.render_widget(paragraph, popup_area);
}
