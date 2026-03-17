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

/// Minimum terminal dimensions for the TUI to render properly.
const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 12;

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();

    // Guard: terminal too small to render meaningful UI
    if size.width < MIN_WIDTH || size.height < MIN_HEIGHT {
        let msg = format!(
            "Terminal too small ({}x{}). Need {}x{} minimum.",
            size.width, size.height, MIN_WIDTH, MIN_HEIGHT
        );
        let paragraph =
            Paragraph::new(msg).style(Style::default().fg(theme::BRIGHT_YELLOW).bg(theme::BG));
        f.render_widget(paragraph, size);
        return;
    }

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

    // Use pre-computed header info — all strings cached via sync_header_info()
    let hi = &app.header_info;

    let label_style = Style::default().fg(theme::FG4);
    let value_style = Style::default().fg(theme::FG);
    let accent_style = Style::default().fg(theme::BRIGHT_AQUA);

    if is_full {
        render_header_full(
            f, area, hi.cluster_name.as_str(), hi.endpoint.as_str(),
            hi.k8s_version.as_str(), &hi.config_path,
            &hi.version_display, &hi.cluster_count_full,
            label_style, value_style, accent_style,
        );
    } else {
        render_header_compact(
            f, area, hi.cluster_name.as_str(), hi.endpoint.as_str(),
            &hi.version_compact, &hi.cluster_count_compact,
            label_style, value_style, accent_style,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_header_full(
    f: &mut Frame,
    area: Rect,
    cluster_name: &str,
    endpoint_str: &str,
    k8s_ver: &str,
    config_path: &str,
    version_display: &str,
    cluster_count_display: &str,
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

    // Split: left logo | right info (k9s style)
    let logo_width: u16 = 52; // widest LOGO line
    let show_logo = inner.width > logo_width + 30;

    let cols = if show_logo {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(logo_width + 2), Constraint::Min(30)])
            .split(inner)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100)])
            .split(inner)
    };

    // Left: ASCII art logo
    if show_logo && cols.len() > 1 {
        let logo_lines: Vec<Line> = LOGO
            .iter()
            .map(|line| {
                Line::from(Span::styled(
                    format!(" {}", line),
                    Style::default()
                        .fg(theme::BRIGHT_ORANGE)
                        .add_modifier(Modifier::BOLD),
                ))
            })
            .collect();
        let logo_para = Paragraph::new(logo_lines).style(Style::default().bg(theme::BG_HARD));
        f.render_widget(logo_para, cols[0]);
    }

    // Right (or full width): info lines
    let info_area = if show_logo && cols.len() > 1 {
        cols[1]
    } else {
        cols[0]
    };

    let info_lines = vec![
        Line::from(vec![
            Span::styled(" Context:   ", label_style),
            Span::styled(cluster_name, accent_style),
        ]),
        Line::from(vec![
            Span::styled(" Cluster:   ", label_style),
            Span::styled(endpoint_str, value_style),
        ]),
        Line::from(vec![
            Span::styled(" K8s Rev:   ", label_style),
            Span::styled(
                k8s_ver,
                if k8s_ver == "N/A" {
                    label_style
                } else {
                    value_style
                },
            ),
        ]),
        Line::from(vec![
            Span::styled(" ScaleX:    ", label_style),
            Span::styled(
                version_display,
                Style::default()
                    .fg(theme::BRIGHT_ORANGE)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                cluster_count_display,
                Style::default().fg(theme::FG3),
            ),
        ]),
        Line::from(vec![
            Span::styled(" Config:    ", label_style),
            Span::styled(config_path, Style::default().fg(theme::FG3)),
        ]),
    ];

    let para = Paragraph::new(info_lines).style(Style::default().bg(theme::BG_HARD));
    f.render_widget(para, info_area);
}

#[allow(clippy::too_many_arguments)]
fn render_header_compact(
    f: &mut Frame,
    area: Rect,
    cluster_name: &str,
    endpoint_str: &str,
    version_compact: &str,
    cluster_count_compact: &str,
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
            version_compact,
            Style::default().fg(theme::BRIGHT_ORANGE),
        ),
        Span::styled(cluster_name, accent_style),
        Span::styled(
            cluster_count_compact,
            Style::default().fg(theme::FG3),
        ),
    ];

    let line2_spans = vec![
        Span::styled(" Cluster: ", label_style),
        Span::styled(endpoint_str, Style::default().fg(theme::FG4)),
    ];

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

    let visible = &app.render_visible_indices;
    let visible_len = visible.len();

    // Reserve 1 row for scroll indicator when content overflows viewport (US-202)
    let overflows = visible_len > inner.height as usize;
    let (para_area, indicator_area) = if overflows {
        let para_h = inner.height.saturating_sub(1);
        (
            Rect::new(inner.x, inner.y, inner.width, para_h),
            Some(Rect::new(inner.x, inner.y + para_h, inner.width, 1)),
        )
    } else {
        (inner, None)
    };

    // Viewport-only rendering: only build Line objects for visible rows
    let viewport_height = para_area.height as usize;
    let scroll_start = app.sidebar_scroll_offset;
    let scroll_end = (scroll_start + viewport_height).min(visible_len);

    let lines: Vec<Line> = visible[scroll_start..scroll_end]
        .iter()
        .enumerate()
        .map(|(row_in_viewport, &idx)| {
            let vi = scroll_start + row_in_viewport; // absolute visible index
            let node = &app.tree[idx];
            let is_cursor = vi == app.tree_cursor && is_active;

            // Check if this node is the actively selected context (US-005)
            let is_active_selection = match &node.node_type {
                NodeType::Cluster(name) => {
                    // Show ● on cluster node when selected with no namespace filter,
                    // even when expanded (children may not yet be loaded).
                    // When a specific namespace is selected, ● moves to the namespace node.
                    app.selected_cluster.as_ref() == Some(name)
                        && app.selected_namespace.is_none()
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

            let marker = if is_active_selection { "● " } else { "  " };

            // Static indent slices — avoids per-row String allocation from "  ".repeat(depth)
            const INDENTS: [&str; 5] = ["", "  ", "    ", "      ", "        "];
            let indent = INDENTS.get(node.depth).copied().unwrap_or(INDENTS[4]);
            let label_color = match &node.node_type {
                NodeType::Root => theme::BRIGHT_ORANGE,
                NodeType::Cluster(_) => theme::BRIGHT_BLUE,
                NodeType::Namespace { .. } => theme::FG,
                NodeType::InfraHeader => theme::BRIGHT_AQUA,
                NodeType::InfraItem(_) => theme::FG3,
            };

            let (style, marker_style, suffix_bg) = if is_cursor {
                let cursor_style = Style::default()
                    .fg(theme::BG_HARD)
                    .bg(theme::BRIGHT_YELLOW)
                    .add_modifier(Modifier::BOLD);
                (cursor_style, cursor_style, theme::BRIGHT_YELLOW)
            } else if is_active_selection {
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

            // Connection status suffix + health dot (single snapshot lookup for both)
            // Use &str references to avoid per-row String clones
            let (conn_suffix, label_ref): (Option<(&str, Color)>, &str) = match &node.node_type {
                NodeType::Cluster(name) => {
                    // Single snapshot lookup reused for both health dot and namespace count
                    let snap = app.snapshot_index
                        .get(name)
                        .and_then(|&i| app.snapshots.get(i))
                        .or_else(|| app.snapshots.iter().find(|s| &s.name == name));

                    let suffix = match app.cluster_connection_status.get(name) {
                        Some(ConnectionStatus::Discovering) => Some((" [..]", theme::FG4)),
                        Some(ConnectionStatus::Failed(_)) => Some((" [!!]", theme::BRIGHT_RED)),
                        Some(ConnectionStatus::Connected) | None => {
                            snap.map(|s| match s.health {
                                data::HealthStatus::Green => (" ●", theme::BRIGHT_GREEN),
                                data::HealthStatus::Yellow => (" ●", theme::BRIGHT_YELLOW),
                                data::HealthStatus::Red => (" ●", theme::BRIGHT_RED),
                                data::HealthStatus::Unknown => (" ○", theme::FG4),
                            })
                        }
                    };
                    // US-205: namespace count — borrow pre-computed label, no clone
                    let label: &str = if node.expanded {
                        node.ns_count_label.as_deref().unwrap_or(&node.label)
                    } else {
                        &node.label
                    };
                    (suffix, label)
                }
                _ => (None, &node.label),
            };

            // Truncate label to fit sidebar width
            let indent_cols = 2 * node.depth;
            let marker_cols: usize = 2;
            let icon_cols: usize = 2;
            let prefix_cols = indent_cols + marker_cols + icon_cols;
            let suffix_cols = conn_suffix
                .as_ref()
                .map(|(s, _)| s.chars().count())
                .unwrap_or(0);
            let available = (inner.width as usize).saturating_sub(prefix_cols + suffix_cols);
            let label_char_count = label_ref.chars().count();
            // Only allocate when truncation is needed; common case borrows directly
            let display_label: std::borrow::Cow<str> = if label_char_count > available {
                if available > 1 {
                    let truncated: String = label_ref.chars().take(available - 1).collect();
                    format!("{}…", truncated).into()
                } else if available == 1 {
                    "…".into()
                } else {
                    "".into()
                }
            } else {
                label_ref.into()
            };
            let label_cols = label_char_count.min(available);

            let mut spans = vec![
                Span::styled(indent, style),
                Span::styled(marker, marker_style),
                Span::styled(icon, style),
                Span::styled(display_label, style),
            ];
            let mut used_cols = prefix_cols + label_cols;
            if let Some((suffix, color)) = conn_suffix {
                used_cols += suffix.chars().count();
                spans.push(Span::styled(
                    suffix,
                    Style::default().fg(color).bg(suffix_bg),
                ));
            }
            // Pad to full sidebar width so cursor/selection highlight fills the row.
            // Static buffer avoids per-row heap allocation from " ".repeat(pad).
            const SPACES: &str = "                                                                                ";
            let pad = (inner.width as usize).saturating_sub(used_cols);
            if pad > 0 {
                let pad_style = if is_cursor {
                    style
                } else {
                    Style::default().bg(theme::BG_HARD)
                };
                if pad <= SPACES.len() {
                    spans.push(Span::styled(&SPACES[..pad], pad_style));
                } else {
                    // Fallback for extremely wide terminals (>80 col sidebar)
                    spans.push(Span::styled(" ".repeat(pad), pad_style));
                };
            }

            Line::from(spans)
        })
        .collect();

    // No scroll offset — we already built only viewport-visible lines
    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, para_area);

    // Scroll indicator on dedicated line (never overlaps tree content)
    // Uses pre-computed string from app.sync_sidebar_indicator() — no per-frame format!()
    if let Some(ind_area) = indicator_area {
        let indicator_widget = Paragraph::new(Span::styled(
            app.sidebar_indicator.as_str(),
            Style::default().fg(theme::FG4).bg(theme::BG_HARD),
        ));
        f.render_widget(indicator_widget, ind_area);
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

    // Resource shortcut indicator: p d s c n with active one highlighted
    // Static strings avoid per-frame format!() allocation for tab labels
    const SHORTCUTS_ACTIVE: [&str; 6] = ["[p]Pods ", "[d]Deploy ", "[s]Svc ", "[c]CM ", "[n]Nodes ", "[e]Events "];
    const SHORTCUTS_INACTIVE: [&str; 6] = ["p:Pods ", "d:Deploy ", "s:Svc ", "c:CM ", "n:Nodes ", "e:Events "];
    const SHORTCUT_VIEWS: [ResourceView; 6] = [
        ResourceView::Pods,
        ResourceView::Deployments,
        ResourceView::Services,
        ResourceView::ConfigMaps,
        ResourceView::Nodes,
        ResourceView::Events,
    ];
    let mut title_spans: Vec<Span> = vec![Span::styled(" ", Style::default().fg(theme::FG))];
    if app.active_tab == 0 {
        for i in 0..6 {
            if SHORTCUT_VIEWS[i] == app.resource_view {
                title_spans.push(Span::styled(
                    SHORTCUTS_ACTIVE[i],
                    Style::default()
                        .fg(theme::BG_HARD)
                        .bg(theme::BRIGHT_AQUA)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                title_spans.push(Span::styled(
                    SHORTCUTS_INACTIVE[i],
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
    // [cached] indicator removed — is_view_stale always returns false with full prefetch.
    // Row count indicator — uses pre-computed string from app.sync_row_count_indicator()
    if app.active_tab == 0 && !app.row_count_indicator.is_empty() {
        title_spans.push(Span::styled(
            app.row_count_indicator.as_str(),
            Style::default().fg(theme::FG3),
        ));
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
        app.ctx_title_span.as_str(),
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
///
/// Priority: cached snapshot data > connection failure > loading/discovery states.
/// When cached data exists, it is always returned even if the cluster connection
/// has since failed — the caller renders data with a separate error banner.
fn render_tab_preamble<'a>(
    f: &mut Frame,
    app: &'a App,
    area: Rect,
) -> Option<&'a data::ClusterSnapshot> {
    // Cached data takes priority — return it even if connection is now failed.
    // The caller will render a separate error banner via render_connection_error_banner().
    if let Some(s) = app.current_snapshot() {
        return Some(s);
    }

    // No cached data — check for connection failure (full-area error)
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

    // Static spinner strings — avoids per-frame format!() allocation
    const PREAMBLE_DISCOVER: [&str; 4] = [
        "  | Discovering clusters...",
        "  / Discovering clusters...",
        "  - Discovering clusters...",
        "  \\ Discovering clusters...",
    ];
    const PREAMBLE_LOADING: [&str; 4] = [
        "  | Loading cluster data...",
        "  / Loading cluster data...",
        "  - Loading cluster data...",
        "  \\ Loading cluster data...",
    ];
    const PREAMBLE_GENERIC: [&str; 4] = [
        "  | Loading...",
        "  / Loading...",
        "  - Loading...",
        "  \\ Loading...",
    ];
    const PREAMBLE_WAITING: [&str; 4] = [
        "  | Waiting for data...",
        "  / Waiting for data...",
        "  - Waiting for data...",
        "  \\ Waiting for data...",
    ];
    let spin_idx = (app.tick_count as usize) % 4;
    let msg: &str = if !app.discover_complete {
        PREAMBLE_DISCOVER[spin_idx]
    } else if app.snapshots.is_empty() && app.is_fetching {
        PREAMBLE_LOADING[spin_idx]
    } else if app.all_clusters_failed() {
        "  All clusters failed to connect. Check sidebar for details. Press 'r' to retry."
    } else if app.snapshots.is_empty() && app.clusters.is_empty() {
        "  No clusters found. Run 'scalex cluster init' first."
    } else if app.is_fetching || (app.selected_cluster.is_some() && app.needs_refresh) {
        PREAMBLE_GENERIC[spin_idx]
    } else if app.selected_cluster.is_some() {
        PREAMBLE_WAITING[spin_idx]
    } else {
        "  Select a cluster and press Enter"
    };
    let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
    f.render_widget(paragraph, area);
    None
}

/// Render a 1-line connection error banner if the selected cluster has failed.
/// Returns the remaining area below the banner (or full area if no error).
fn render_connection_error_banner(f: &mut Frame, app: &App, area: Rect) -> Rect {
    if let Some(cluster_name) = &app.selected_cluster {
        if let Some(ConnectionStatus::Failed(err_msg)) =
            app.cluster_connection_status.get(cluster_name)
        {
            let banner_height = 1;
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(banner_height),
                    Constraint::Min(0),
                ])
                .split(area);

            let banner = Line::from(vec![
                Span::styled(" [!!] ", Style::default().fg(theme::BRIGHT_RED)),
                Span::styled(
                    format!("{} — press 'r' to retry", err_msg),
                    Style::default().fg(theme::BRIGHT_RED),
                ),
            ]);
            let paragraph =
                Paragraph::new(banner).style(Style::default().bg(theme::BG));
            f.render_widget(paragraph, chunks[0]);
            return chunks[1];
        }
    }
    area
}

fn render_resources_tab(f: &mut Frame, app: &App, area: Rect) {
    let snapshot = match render_tab_preamble(f, app, area) {
        Some(s) => s,
        None => return,
    };

    // Show error banner if cluster connection failed (but cached data exists)
    let content_area = render_connection_error_banner(f, app, area);

    // Check if the current resource type has been fetched yet.
    // If not, show a loading indicator instead of an empty table.
    let active = app.resource_view.to_active_resource();
    let is_empty = match app.resource_view {
        ResourceView::Pods => snapshot.pods.is_empty(),
        ResourceView::Deployments => snapshot.deployments.is_empty(),
        ResourceView::Services => snapshot.services.is_empty(),
        ResourceView::ConfigMaps => snapshot.configmaps.is_empty(),
        ResourceView::Nodes => snapshot.nodes.is_empty(),
        ResourceView::Events => snapshot.events.is_empty(),
    };
    if is_empty && !app.fetched_resources.contains(&active) {
        // Static per-resource loading spinners — avoids per-frame format!()
        const LOAD_PODS: [&str; 4] = ["  | Loading Pods...", "  / Loading Pods...", "  - Loading Pods...", "  \\ Loading Pods..."];
        const LOAD_DEPLOY: [&str; 4] = ["  | Loading Deployments...", "  / Loading Deployments...", "  - Loading Deployments...", "  \\ Loading Deployments..."];
        const LOAD_SVC: [&str; 4] = ["  | Loading Services...", "  / Loading Services...", "  - Loading Services...", "  \\ Loading Services..."];
        const LOAD_CM: [&str; 4] = ["  | Loading ConfigMaps...", "  / Loading ConfigMaps...", "  - Loading ConfigMaps...", "  \\ Loading ConfigMaps..."];
        const LOAD_NODES: [&str; 4] = ["  | Loading Nodes...", "  / Loading Nodes...", "  - Loading Nodes...", "  \\ Loading Nodes..."];
        const LOAD_EVENTS: [&str; 4] = ["  | Loading Events...", "  / Loading Events...", "  - Loading Events...", "  \\ Loading Events..."];
        let spin_idx = (app.tick_count as usize) % 4;
        let msg = match app.resource_view {
            ResourceView::Pods => LOAD_PODS[spin_idx],
            ResourceView::Deployments => LOAD_DEPLOY[spin_idx],
            ResourceView::Services => LOAD_SVC[spin_idx],
            ResourceView::ConfigMaps => LOAD_CM[spin_idx],
            ResourceView::Nodes => LOAD_NODES[spin_idx],
            ResourceView::Events => LOAD_EVENTS[spin_idx],
        };
        let paragraph = Paragraph::new(msg).style(Style::default().fg(theme::FG4));
        f.render_widget(paragraph, content_area);
        return;
    }

    match app.resource_view {
        ResourceView::Pods => render_pods_table(f, app, &snapshot.pods, content_area),
        ResourceView::Deployments => {
            render_deployments_table(f, app, &snapshot.deployments, content_area)
        }
        ResourceView::Services => render_services_table(f, app, &snapshot.services, content_area),
        ResourceView::Nodes => render_nodes_table(f, app, &snapshot.nodes, content_area),
        ResourceView::ConfigMaps => {
            render_configmaps_table(f, app, &snapshot.configmaps, content_area)
        }
        ResourceView::Events => render_events_table(f, app, &snapshot.events, content_area),
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
    // Use cached row count from App when available; fall back to compute.
    // This avoids the previous double-iteration (count + render).
    let filtered_count = app.current_row_count_readonly();

    if filtered_count == 0 {
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

    let clamped_cursor = app.table_cursor.min(filtered_count - 1);
    let viewport_rows = area.height.saturating_sub(1) as usize;
    // Defensively clamp scroll offset to prevent blank rows (US-211)
    let clamped_scroll = app
        .table_scroll_offset
        .min(filtered_count.saturating_sub(viewport_rows.max(1)));
    // Build Row objects only for viewport-visible items (skip+take on filtered iterator).
    // Single-pass: filter + enumerate + skip/take + row construction.
    let rows: Vec<Row> = items
        .iter()
        .filter(|item| filter_fn(item))
        .enumerate()
        .skip(clamped_scroll)
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
        resource_header(vec![
            "NAME",
            "NAMESPACE",
            "STATUS",
            "READY",
            "RESTARTS",
            "AGE",
            "NODE",
        ]),
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
                "Failed"
                | "CrashLoopBackOff"
                | "Error"
                | "OOMKilled"
                | "ImagePullBackOff"
                | "ErrImagePull"
                | "CreateContainerConfigError"
                | "InvalidImageName"
                | "Evicted"
                | "NodeLost"
                | "Shutdown" => theme::BRIGHT_RED,
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
                Cell::from(pod.restarts_display.as_str()).style(restart_style),
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
        resource_header(vec![
            "NAME",
            "NAMESPACE",
            "READY",
            "UP-TO-DATE",
            "AVAILABLE",
            "AGE",
        ]),
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
                let (ready, desired) = (dep.ready_count, dep.desired_count);
                let color = if desired == 0 {
                    theme::FG4 // scaled-to-zero: neutral/dim, not green
                } else if ready >= desired {
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
                Cell::from(dep.up_to_date_display.as_str()).style(base),
                Cell::from(dep.available_display.as_str()).style(base),
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
        resource_header(vec![
            "NAME",
            "NAMESPACE",
            "TYPE",
            "CLUSTER-IP",
            "EXTERNAL-IP",
            "PORTS",
            "AGE",
        ]),
        &[
            Constraint::Min(16),
            Constraint::Min(10),
            Constraint::Length(12),
            Constraint::Length(16),
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
                Cell::from(svc.external_ip.as_str()).style(base),
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
        resource_header(vec!["NAME", "STATUS", "ROLES", "VERSION", "CPU", "MEMORY", "AGE"]),
        &[
            Constraint::Min(16),
            Constraint::Length(10),
            Constraint::Min(10),
            Constraint::Length(10),
            Constraint::Min(10),
            Constraint::Min(14),
            Constraint::Length(6),
        ],
        "No nodes found",
        |node| app.matches_search(&node.name),
        |_i, node, is_selected| {
            let base = row_base_style(is_selected);
            // Use pre-computed display strings (avoids per-frame format! + format_k8s_memory + join)
            if is_selected {
                Row::new(vec![
                    Cell::from(node.name.as_str()).style(base),
                    Cell::from(node.status.as_str()).style(base),
                    Cell::from(node.roles_display.as_str()).style(base),
                    Cell::from(node.kubelet_version.as_str()).style(base),
                    Cell::from(node.cpu_display.as_str()).style(base),
                    Cell::from(node.mem_display.as_str()).style(base),
                    Cell::from(node.age.as_str()).style(base),
                ])
            } else {
                let status_color = if node.status == "Ready" {
                    theme::BRIGHT_GREEN
                } else if node.status.contains("SchedulingDisabled") {
                    theme::BRIGHT_YELLOW
                } else {
                    theme::BRIGHT_RED
                };
                Row::new(vec![
                    Cell::from(node.name.as_str()).style(Style::default().fg(theme::FG)),
                    Cell::from(node.status.as_str()).style(Style::default().fg(status_color)),
                    Cell::from(node.roles_display.as_str()).style(Style::default().fg(theme::FG3)),
                    Cell::from(node.kubelet_version.as_str()).style(Style::default().fg(theme::FG4)),
                    Cell::from(node.cpu_display.as_str()).style(Style::default().fg(theme::BRIGHT_AQUA)),
                    Cell::from(node.mem_display.as_str()).style(Style::default().fg(theme::BRIGHT_PURPLE)),
                    Cell::from(node.age.as_str()).style(Style::default().fg(theme::FG3)),
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
                Cell::from(cm.data_keys_display.as_str()).style(base),
                Cell::from(cm.age.as_str()).style(base),
            ])
        },
    );
}

fn render_events_table(f: &mut Frame, app: &App, events: &[data::EventInfo], area: Rect) {
    render_resource_table(
        f,
        app,
        events,
        area,
        resource_header(vec![
            "NAMESPACE",
            "LAST SEEN",
            "TYPE",
            "REASON",
            "OBJECT",
            "MESSAGE",
        ]),
        &[
            Constraint::Min(12),    // NAMESPACE
            Constraint::Length(8),  // LAST SEEN
            Constraint::Length(8),  // TYPE
            Constraint::Length(16), // REASON
            Constraint::Min(20),   // OBJECT
            Constraint::Min(30),   // MESSAGE
        ],
        "No events",
        |evt| {
            app.matches_search_with_ns(&evt.reason, &evt.namespace)
                || app.matches_search(&evt.object)
                || app.matches_search(&evt.message)
        },
        |_i, evt, is_selected| {
            let base = row_base_style(is_selected);
            let is_warning = evt.event_type == "Warning";
            let type_color = if is_warning {
                theme::BRIGHT_YELLOW
            } else {
                theme::FG3
            };
            Row::new(vec![
                Cell::from(evt.namespace.as_str()).style(base),
                Cell::from(evt.last_seen.as_str()).style(base),
                Cell::from(evt.event_type.as_str()).style(if is_selected {
                    base
                } else {
                    Style::default().fg(type_color)
                }),
                Cell::from(evt.reason.as_str()).style(base),
                Cell::from(evt.object.as_str()).style(base),
                Cell::from(evt.message.as_str()).style(base),
            ])
        },
    );
}

fn render_top_tab(f: &mut Frame, app: &App, area: Rect) {
    let snapshot = match render_tab_preamble(f, app, area) {
        Some(s) => s,
        None => return,
    };

    // Show error banner if cluster connection failed (but cached data exists)
    let content_area = render_connection_error_banner(f, app, area);
    let area = content_area;

    // US-902: Show "Utilization" only when metrics data available, else "Resources"
    let has_metrics = snapshot.resource_usage.cpu_percent >= 0.0;
    let top_title = if has_metrics {
        " Node Resource Utilization "
    } else {
        " Node Resources (no metrics) "
    };

    let mut lines = vec![
        Line::from(vec![Span::styled(
            top_title,
            Style::default()
                .fg(theme::BRIGHT_YELLOW)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
    ];

    // Static status icon strings — avoids per-node format!() allocation
    const ICON_READY: &str = " ● ";
    const ICON_OTHER: &str = " ○ ";

    // Iterate directly without collecting into Vec (avoids per-frame allocation)
    let mut has_nodes = false;
    for node in snapshot.nodes.iter().filter(|n| app.matches_search(&n.name)) {
        has_nodes = true;
        let (status_icon, status_color) = if node.status.starts_with("Ready") {
            if node.status.contains("SchedulingDisabled") {
                (ICON_READY, theme::BRIGHT_YELLOW)
            } else {
                (ICON_READY, theme::BRIGHT_GREEN)
            }
        } else {
            (ICON_OTHER, theme::BRIGHT_RED)
        };

        lines.push(Line::from(vec![
            Span::styled(status_icon, Style::default().fg(status_color)),
            Span::styled(
                &node.name,
                Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                node.top_display.as_str(),
                Style::default().fg(theme::FG3),
            ),
        ]));
    }

    if !has_nodes {
        let msg = if app.search_query.as_ref().is_some_and(|q| !q.is_empty()) {
            format!(
                " No results for \"{}\"",
                app.search_query.as_deref().unwrap_or("")
            )
        } else {
            " No node data available".to_string()
        };
        lines.push(Line::from(Span::styled(
            msg,
            Style::default().fg(theme::FG4),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(theme::BG))
        .scroll((app.table_scroll_offset.min(u16::MAX as usize) as u16, 0));
    f.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

/// Render a compact utilization bar: `CPU [========--] 82%` or `CPU N/A`
/// Static fill/empty bar strings — indexed by length to avoid per-frame String::repeat() allocation.
const BAR_FILL: &str = "===================="; // 20 chars max
const BAR_EMPTY: &str = "--------------------"; // 20 chars max

/// Static lookup table for "] XXX% " suffix strings (0-100).
/// Avoids per-frame format!() allocation in render_usage_bar.
const PERCENT_SUFFIXES: [&str; 101] = [
    "]   0% ", "]   1% ", "]   2% ", "]   3% ", "]   4% ", "]   5% ", "]   6% ", "]   7% ", "]   8% ", "]   9% ",
    "]  10% ", "]  11% ", "]  12% ", "]  13% ", "]  14% ", "]  15% ", "]  16% ", "]  17% ", "]  18% ", "]  19% ",
    "]  20% ", "]  21% ", "]  22% ", "]  23% ", "]  24% ", "]  25% ", "]  26% ", "]  27% ", "]  28% ", "]  29% ",
    "]  30% ", "]  31% ", "]  32% ", "]  33% ", "]  34% ", "]  35% ", "]  36% ", "]  37% ", "]  38% ", "]  39% ",
    "]  40% ", "]  41% ", "]  42% ", "]  43% ", "]  44% ", "]  45% ", "]  46% ", "]  47% ", "]  48% ", "]  49% ",
    "]  50% ", "]  51% ", "]  52% ", "]  53% ", "]  54% ", "]  55% ", "]  56% ", "]  57% ", "]  58% ", "]  59% ",
    "]  60% ", "]  61% ", "]  62% ", "]  63% ", "]  64% ", "]  65% ", "]  66% ", "]  67% ", "]  68% ", "]  69% ",
    "]  70% ", "]  71% ", "]  72% ", "]  73% ", "]  74% ", "]  75% ", "]  76% ", "]  77% ", "]  78% ", "]  79% ",
    "]  80% ", "]  81% ", "]  82% ", "]  83% ", "]  84% ", "]  85% ", "]  86% ", "]  87% ", "]  88% ", "]  89% ",
    "]  90% ", "]  91% ", "]  92% ", "]  93% ", "]  94% ", "]  95% ", "]  96% ", "]  97% ", "]  98% ", "]  99% ",
    "] 100% ",
];

fn render_usage_bar<'a>(label: &'a str, percent: f64, width: usize, color: Color) -> Vec<Span<'a>> {
    if percent < 0.0 {
        return vec![
            Span::styled(label, Style::default().fg(theme::FG4)),
            Span::styled(" N/A ", Style::default().fg(theme::FG4)),
        ];
    }
    let width = width.min(BAR_FILL.len());
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

    let pct_idx = (percent.round() as usize).min(100);
    vec![
        Span::styled(label, Style::default().fg(theme::FG4)),
        Span::styled(" [", Style::default().fg(theme::FG4)),
        Span::styled(&BAR_FILL[..filled], Style::default().fg(bar_color)),
        Span::styled(&BAR_EMPTY[..empty], Style::default().fg(theme::FG4)),
        Span::styled(
            PERCENT_SUFFIXES[pct_idx],
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
    // Static spinner strings — avoids per-frame format!() for spinner animation
    const SPINNERS: [&str; 4] = [" | ", " / ", " - ", " \\ "];
    const DISCOVER_SPINNERS: [&str; 4] = [
        " | Discovering clusters...",
        " / Discovering clusters...",
        " - Discovering clusters...",
        " \\ Discovering clusters...",
    ];
    const LOADING_SPINNERS: [&str; 4] = [
        " | Loading cluster data...",
        " / Loading cluster data...",
        " - Loading cluster data...",
        " \\ Loading cluster data...",
    ];
    let spin_idx = (app.tick_count as usize) % 4;

    let mut health_spans: Vec<Span> = if !app.discover_complete {
        vec![Span::styled(
            DISCOVER_SPINNERS[spin_idx],
            Style::default().fg(theme::BRIGHT_YELLOW),
        )]
    } else if app.snapshots.is_empty() && app.is_fetching {
        vec![Span::styled(
            LOADING_SPINNERS[spin_idx],
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
    // Static health dot strings — avoids per-cluster format!() allocation per frame
    const DOT_FILLED: &str = "● ";
    const DOT_EMPTY: &str = "○ ";
    for (i, snapshot) in app.snapshots.iter().enumerate() {
        let (dot_str, color) = match snapshot.health {
            HealthStatus::Green => (DOT_FILLED, theme::BRIGHT_GREEN),
            HealthStatus::Yellow => (DOT_FILLED, theme::BRIGHT_YELLOW),
            HealthStatus::Red => (DOT_FILLED, theme::BRIGHT_RED),
            HealthStatus::Unknown => (DOT_EMPTY, theme::FG4),
        };
        health_spans.push(Span::styled(
            dot_str,
            Style::default().fg(color),
        ));
        // Use pre-computed health strings (computed on fetch arrival, not per-frame)
        if let Some((narrow_str, wide_str, _)) = app.status_bar_health_strings.get(i) {
            let text = if narrow { narrow_str.as_str() } else { wide_str.as_str() };
            health_spans.push(Span::styled(text, Style::default().fg(theme::FG3)));
        }
    }

    // Line 2: CPU/Mem bars per cluster + self overhead + latency
    let mut usage_spans: Vec<Span> = vec![Span::styled(" ", Style::default().fg(theme::FG4))];
    let very_narrow = inner.width < 60;

    // Self overhead + fetch indicator (always shown)
    // rss_str and latency are pre-computed in status_bar_self_line; spinner appended dynamically

    if !very_narrow {
        let bar_width = if narrow { 5 } else { 8 };
        for (i, snapshot) in app.snapshots.iter().enumerate() {
            let ru = &snapshot.resource_usage;
            // Use pre-computed "name: " label to avoid per-frame format!()
            let name_label = app.status_bar_health_strings.get(i)
                .map(|(_, _, nl)| nl.as_str())
                .unwrap_or("");
            usage_spans.push(Span::styled(
                name_label,
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
    }

    // Pre-computed self/latency base string; append spinner dynamically if fetching
    usage_spans.push(Span::styled(
        app.status_bar_self_line.as_str(),
        Style::default().fg(theme::BRIGHT_AQUA),
    ));
    if app.is_fetching {
        usage_spans.push(Span::styled(
            SPINNERS[spin_idx],
            Style::default().fg(theme::BRIGHT_AQUA),
        ));
    }
    if app.fetch_timed_out {
        usage_spans.push(Span::styled(
            " [!] fetch timed out — press 'r' to retry",
            Style::default().fg(theme::BRIGHT_RED),
        ));
    }

    let mut status_text = vec![Line::from(health_spans), Line::from(usage_spans)];

    // Line 3: discovery log message (auto-fading)
    if let Some(log_msg) = app.latest_discovery_log() {
        status_text.push(Line::from(Span::styled(
            format!(" {}", log_msg),
            Style::default().fg(theme::FG4),
        )));
    }

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
                    lines.push(key("p d s c n e", "Switch resource view"));
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
    lines.push(key("1/2", "Switch to tab (Resources/Top)"));
    lines.push(key("/", "Search (filter by name/namespace)"));
    lines.push(key("ESC", "Clear active filter / close overlay"));
    lines.push(key("r", "Force refresh"));
    lines.push(key("?", "Toggle this help"));

    // Footer
    lines.push(Line::from(""));
    let footer_text =
        if app.search_query.as_ref().is_some_and(|q| !q.is_empty()) && !app.search_active {
            "  Press ESC to clear filter, ? to close"
        } else {
            "  Press ESC or ? to close"
        };
    lines.push(Line::from(Span::styled(
        footer_text.to_string(),
        Style::default().fg(theme::FG4),
    )));

    // k9s attribution
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Inspired by k9s (github.com/derailed/k9s)".to_string(),
        Style::default().fg(Color::DarkGray),
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
