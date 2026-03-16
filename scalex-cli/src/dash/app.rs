use crate::commands::dash::DashArgs;
use crate::dash::data::{self, ActiveResource, ClusterSnapshot, HealthStatus};
use crate::dash::event::{self, AppEvent};
use crate::dash::infra::{self, InfraSnapshot};
use crate::dash::kube_client::{self, ClusterClient};
use crate::dash::ui;
use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePanel {
    Sidebar,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceView {
    Pods,
    Deployments,
    Services,
    ConfigMaps,
    Nodes,
}

impl ResourceView {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pods => "Pods",
            Self::Deployments => "Deployments",
            Self::Services => "Services",
            Self::ConfigMaps => "ConfigMaps",
            Self::Nodes => "Nodes",
        }
    }

    pub fn from_char(c: char) -> Option<Self> {
        match c {
            'p' => Some(Self::Pods),
            'd' => Some(Self::Deployments),
            's' => Some(Self::Services),
            'c' => Some(Self::ConfigMaps),
            'n' => Some(Self::Nodes),
            _ => None,
        }
    }

    pub fn to_active_resource(self) -> ActiveResource {
        match self {
            Self::Pods => ActiveResource::Pods,
            Self::Deployments => ActiveResource::Deployments,
            Self::Services => ActiveResource::Services,
            Self::ConfigMaps => ActiveResource::ConfigMaps,
            Self::Nodes => ActiveResource::Nodes,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Tab {
    pub name: String,
    pub closable: bool,
}

// ---------------------------------------------------------------------------
// Tree node for sidebar
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub label: String,
    pub depth: usize,
    pub expanded: bool,
    pub node_type: NodeType,
    pub children_loaded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeType {
    Root,
    Cluster(String),
    Namespace { cluster: String, namespace: String },
    InfraHeader,
    InfraItem(String),
}

// ---------------------------------------------------------------------------
// Connection status for per-cluster discovery tracking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Discovering,
    Connected,
    Failed(String),
}

/// Result of a background data fetch
pub struct FetchResult {
    pub snapshots: Vec<ClusterSnapshot>,
    pub latency_ms: u64,
    /// Which resource was selectively fetched (None = full fetch)
    pub active_resource: Option<ActiveResource>,
    /// Generation counter to detect stale results
    pub generation: u64,
}

// ---------------------------------------------------------------------------
// App struct
// ---------------------------------------------------------------------------

pub struct App {
    pub running: bool,
    pub active_panel: ActivePanel,
    pub active_tab: usize,
    pub tabs: Vec<Tab>,
    pub resource_view: ResourceView,

    // Sidebar tree
    pub tree: Vec<TreeNode>,
    pub tree_cursor: usize,
    pub sidebar_scroll_offset: usize,

    // Center table scroll
    pub table_cursor: usize,
    pub table_scroll_offset: usize,

    // Selected context
    pub selected_cluster: Option<String>,
    pub selected_namespace: Option<String>,

    // Data (clusters kept for future tab-management features)
    #[allow(dead_code)]
    pub clusters: Vec<ClusterClient>,
    pub snapshots: Vec<ClusterSnapshot>,
    pub infra: InfraSnapshot,

    // Timing
    pub api_latency_ms: u64,

    // Help overlay
    pub show_help: bool,
    pub help_scroll_offset: u16,

    // Search
    pub search_active: bool,
    pub search_query: Option<String>,

    // Self-monitoring (sampled at refresh interval, not per-tick)
    pub self_rss_mb: Option<f64>,

    #[allow(dead_code)]
    pub refresh_secs: u64,

    /// Set to true to force a data refresh on next tick (e.g., on view switch)
    pub needs_refresh: bool,

    // --- Non-blocking architecture fields (v2) ---
    /// Per-cluster connection status during discovery
    pub cluster_connection_status: HashMap<String, ConnectionStatus>,

    /// True when background discover_clusters_streaming has sent Complete
    pub discover_complete: bool,

    /// SSH tunnel PIDs accumulated from discovered clusters (for cleanup)
    pub tunnel_pids: Vec<u32>,

    /// True when a background fetch task is in-flight
    pub is_fetching: bool,

    /// When the current fetch started (for 30s timeout defense)
    pub fetch_started_at: Option<Instant>,

    /// Monotonic tick counter for spinner animation
    pub tick_count: u64,

    /// Which resource was last fetched (for staleness indicator in UI)
    pub last_fetched_resource: Option<ActiveResource>,

    /// Monotonic generation counter — incremented on every navigation/view change.
    /// Fetch results with a stale generation are discarded.
    pub fetch_generation: u64,

    /// True when a fetch timed out (30s) — triggers warning in status bar
    pub fetch_timed_out: bool,

    /// Cached table viewport height for PageUp/PageDown (set from run_tui before each render)
    pub page_size: usize,
    /// Cached sidebar viewport height for PageUp/PageDown
    pub sidebar_page_size: usize,
    /// Cached help popup inner height for scroll clamping (US-204)
    pub help_viewport_height: u16,
}

impl App {
    #[cfg(test)]
    pub fn new(clusters: Vec<ClusterClient>, refresh_secs: u64) -> Self {
        let mut tree = vec![TreeNode {
            label: "ScaleX".to_string(),
            depth: 0,
            expanded: true,
            node_type: NodeType::Root,
            children_loaded: true,
        }];

        // Add cluster nodes
        for c in &clusters {
            tree.push(TreeNode {
                label: c.name.clone(),
                depth: 1,
                expanded: false,
                node_type: NodeType::Cluster(c.name.clone()),
                children_loaded: false,
            });
        }

        // Add infrastructure header
        tree.push(TreeNode {
            label: "Infrastructure".to_string(),
            depth: 0,
            expanded: false,
            node_type: NodeType::InfraHeader,
            children_loaded: false,
        });

        let tabs = vec![
            Tab {
                name: "Resources".to_string(),
                closable: false,
            },
            Tab {
                name: "Top".to_string(),
                closable: false,
            },
        ];

        Self {
            running: true,
            active_panel: ActivePanel::Sidebar,
            active_tab: 0,
            tabs,
            resource_view: ResourceView::Pods,
            tree,
            tree_cursor: 0,
            sidebar_scroll_offset: 0,
            table_cursor: 0,
            table_scroll_offset: 0,
            selected_cluster: None,
            selected_namespace: None,
            clusters,
            snapshots: Vec::new(),
            infra: InfraSnapshot::default(),
            api_latency_ms: 0,
            show_help: false,
            help_scroll_offset: 0,
            search_active: false,
            search_query: None,
            self_rss_mb: None,
            refresh_secs,
            needs_refresh: false,
            cluster_connection_status: HashMap::new(),
            discover_complete: true, // already have clients
            tunnel_pids: Vec::new(),
            is_fetching: false,
            fetch_started_at: None,
            tick_count: 0,
            last_fetched_resource: None,
            fetch_generation: 0,
            fetch_timed_out: false,
            page_size: 0,
            sidebar_page_size: 0,
            help_viewport_height: 0,
        }
    }

    /// Create App with cluster names only (no clients yet).
    /// Used for non-blocking TUI startup — clients arrive via channel later.
    pub fn new_with_names(cluster_names: &[String], refresh_secs: u64) -> Self {
        let mut tree = vec![TreeNode {
            label: "ScaleX".to_string(),
            depth: 0,
            expanded: true,
            node_type: NodeType::Root,
            children_loaded: true,
        }];

        let mut cluster_connection_status = HashMap::new();

        for name in cluster_names {
            tree.push(TreeNode {
                label: name.clone(),
                depth: 1,
                expanded: false,
                node_type: NodeType::Cluster(name.clone()),
                children_loaded: false,
            });
            cluster_connection_status.insert(name.clone(), ConnectionStatus::Discovering);
        }

        tree.push(TreeNode {
            label: "Infrastructure".to_string(),
            depth: 0,
            expanded: false,
            node_type: NodeType::InfraHeader,
            children_loaded: false,
        });

        let tabs = vec![
            Tab {
                name: "Resources".to_string(),
                closable: false,
            },
            Tab {
                name: "Top".to_string(),
                closable: false,
            },
        ];

        Self {
            running: true,
            active_panel: ActivePanel::Sidebar,
            active_tab: 0,
            tabs,
            resource_view: ResourceView::Pods,
            tree,
            tree_cursor: 0,
            sidebar_scroll_offset: 0,
            table_cursor: 0,
            table_scroll_offset: 0,
            selected_cluster: None,
            selected_namespace: None,
            clusters: Vec::new(),
            snapshots: Vec::new(),
            infra: InfraSnapshot::default(),
            api_latency_ms: 0,
            show_help: false,
            help_scroll_offset: 0,
            search_active: false,
            search_query: None,
            self_rss_mb: None,
            refresh_secs,
            needs_refresh: false,
            cluster_connection_status,
            discover_complete: false,
            tunnel_pids: Vec::new(),
            is_fetching: false,
            fetch_started_at: None,
            tick_count: 0,
            last_fetched_resource: None,
            fetch_generation: 0,
            fetch_timed_out: false,
            page_size: 0,
            sidebar_page_size: 0,
            help_viewport_height: 0,
        }
    }

    pub fn handle_event(&mut self, evt: AppEvent) {
        // ForceQuit (Ctrl+C) always exits regardless of mode
        if matches!(evt, AppEvent::ForceQuit) {
            self.running = false;
            return;
        }

        // Search-mode: intercept all events as text input (mirrors show_help pattern)
        if self.search_active {
            match evt {
                AppEvent::Enter => {
                    // Submit search — keep query as filter, exit search mode
                    self.search_active = false;
                    // Clean up empty query to None (no filter)
                    if self.search_query.as_ref().is_some_and(|q| q.is_empty()) {
                        self.search_query = None;
                    }
                }
                AppEvent::Escape => {
                    // Cancel search — clear query and exit
                    self.search_active = false;
                    self.search_query = None;
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::Backspace => {
                    // Delete last character
                    if let Some(q) = &mut self.search_query {
                        q.pop();
                        if q.is_empty() {
                            self.search_query = None;
                        }
                    }
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                // Arrow keys, page keys, Home/End in search mode: no-op (don't type characters)
                AppEvent::ArrowUp
                | AppEvent::ArrowDown
                | AppEvent::ArrowLeft
                | AppEvent::ArrowRight
                | AppEvent::PageUp
                | AppEvent::PageDown
                | AppEvent::Home
                | AppEvent::End => {}
                // Tab/Shift+Tab: cancel search (clear query) and switch panel
                AppEvent::NextPanel | AppEvent::PrevPanel => {
                    self.search_active = false;
                    self.search_query = None;
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                    self.active_panel = match self.active_panel {
                        ActivePanel::Sidebar => ActivePanel::Center,
                        ActivePanel::Center => ActivePanel::Sidebar,
                    };
                }
                // All character-producing events → literal text input
                // Vim keys (q→Quit, h→Left, l→Right, ?→Help) are remapped to chars
                AppEvent::Quit => {
                    self.search_query.get_or_insert_with(String::new).push('q');
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::Help => {
                    self.search_query.get_or_insert_with(String::new).push('?');
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::Left => {
                    self.search_query.get_or_insert_with(String::new).push('h');
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::Right => {
                    self.search_query.get_or_insert_with(String::new).push('l');
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::Up => {
                    self.search_query.get_or_insert_with(String::new).push('k');
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::Down => {
                    self.search_query.get_or_insert_with(String::new).push('j');
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::Refresh => {
                    self.search_query.get_or_insert_with(String::new).push('r');
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::Search => {
                    self.search_query.get_or_insert_with(String::new).push('/');
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::ResourceType(c) => {
                    self.search_query.get_or_insert_with(String::new).push(c);
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                AppEvent::CharInput(c) => {
                    self.search_query.get_or_insert_with(String::new).push(c);
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
                _ => {}
            }
            return;
        }

        // show_help and search_active are mutually exclusive by construction:
        // search intercepts Help/Quit (above), and help blocks Search (below).
        if self.show_help {
            match evt {
                AppEvent::Help | AppEvent::Quit | AppEvent::Enter | AppEvent::Escape => {
                    self.show_help = false;
                    self.help_scroll_offset = 0;
                }
                AppEvent::Up | AppEvent::ArrowUp => {
                    self.help_scroll_offset = self.help_scroll_offset.saturating_sub(1);
                }
                AppEvent::Down | AppEvent::ArrowDown => {
                    let max = self.help_max_scroll();
                    if self.help_scroll_offset < max {
                        self.help_scroll_offset += 1;
                    }
                }
                AppEvent::PageUp => {
                    let jump = (self.page_size / 2).max(1) as u16;
                    self.help_scroll_offset = self.help_scroll_offset.saturating_sub(jump);
                }
                AppEvent::PageDown => {
                    let max = self.help_max_scroll();
                    let jump = (self.page_size / 2).max(1) as u16;
                    self.help_scroll_offset = (self.help_scroll_offset + jump).min(max);
                }
                AppEvent::Home => {
                    self.help_scroll_offset = 0;
                }
                AppEvent::End => {
                    self.help_scroll_offset = self.help_max_scroll();
                }
                _ => {}
            }
            return;
        }

        match evt {
            AppEvent::Quit => self.running = false,
            AppEvent::Up | AppEvent::ArrowUp => self.move_up(),
            AppEvent::Down | AppEvent::ArrowDown => self.move_down(),
            AppEvent::PageUp => self.page_up(),
            AppEvent::PageDown => self.page_down(),
            AppEvent::Home => self.jump_home(),
            AppEvent::End => self.jump_end(),
            AppEvent::Enter => self.handle_enter(),
            AppEvent::Left | AppEvent::ArrowLeft => self.collapse_node(),
            AppEvent::Right | AppEvent::ArrowRight => self.expand_node(),
            AppEvent::Tab(n) => {
                if n > 0 && n <= self.tabs.len() {
                    let new_tab = n - 1;
                    if self.active_tab != new_tab {
                        self.active_tab = new_tab;
                        self.table_cursor = 0;
                        self.table_scroll_offset = 0;
                    }
                }
            }
            AppEvent::NextPanel => {
                self.active_panel = match self.active_panel {
                    ActivePanel::Sidebar => ActivePanel::Center,
                    ActivePanel::Center => ActivePanel::Sidebar,
                };
            }
            AppEvent::PrevPanel => {
                self.active_panel = match self.active_panel {
                    ActivePanel::Sidebar => ActivePanel::Center,
                    ActivePanel::Center => ActivePanel::Sidebar,
                };
            }
            AppEvent::ResourceType(c) => {
                // Resource view shortcuts only work when center panel is active
                if self.active_panel == ActivePanel::Center {
                    if let Some(rv) = ResourceView::from_char(c) {
                        if self.resource_view != rv {
                            self.resource_view = rv;
                            self.table_cursor = 0;
                            self.table_scroll_offset = 0;
                            self.needs_refresh = true;
                            self.fetch_generation += 1;
                            self.is_fetching = false;
                        }
                    }
                }
            }
            AppEvent::Help => {
                self.show_help = true;
                self.help_scroll_offset = 0;
            }
            AppEvent::Search => {
                // Activate search mode and switch to center panel (search filters center table)
                self.search_active = true;
                self.search_query = Some(String::new());
                self.active_panel = ActivePanel::Center;
                self.table_cursor = 0;
                self.table_scroll_offset = 0;
            }
            AppEvent::Refresh => {
                self.needs_refresh = true;
                self.fetch_generation += 1;
                self.is_fetching = false;
            }
            AppEvent::Backspace => {} // no-op in normal mode; only used in search mode
            AppEvent::Escape => {
                // Clear active search filter if present
                if self.search_query.as_ref().is_some_and(|q| !q.is_empty()) {
                    self.search_query = None;
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                }
            }
            AppEvent::Tick | AppEvent::None | AppEvent::CharInput(_) => {}
            AppEvent::ForceQuit => {} // handled at top of handle_event
        }
    }

    /// Check if a name matches the current search query (case-insensitive)
    pub fn matches_search(&self, name: &str) -> bool {
        match &self.search_query {
            Some(q) if !q.is_empty() => name.to_lowercase().contains(&q.to_lowercase()),
            _ => true,
        }
    }

    /// Check if a resource matches search by name OR namespace (case-insensitive)
    pub fn matches_search_with_ns(&self, name: &str, namespace: &str) -> bool {
        match &self.search_query {
            Some(q) if !q.is_empty() => {
                let q_lower = q.to_lowercase();
                name.to_lowercase().contains(&q_lower)
                    || namespace.to_lowercase().contains(&q_lower)
            }
            _ => true,
        }
    }

    /// Clamp table_cursor so it never exceeds the last valid index.
    /// Called from UI renderers before checking is_selected, and from move_down.
    pub fn clamp_table_cursor(&mut self, row_count: usize) {
        if row_count == 0 {
            self.table_cursor = 0;
        } else if self.table_cursor >= row_count {
            self.table_cursor = row_count - 1;
        }
    }

    fn move_up(&mut self) {
        match self.active_panel {
            ActivePanel::Sidebar => {
                if self.tree_cursor > 0 {
                    self.tree_cursor -= 1;
                }
                self.adjust_sidebar_scroll();
            }
            ActivePanel::Center => {
                if self.active_tab == 1 {
                    // Top tab: line-by-line scroll (Paragraph, no row selection)
                    self.table_scroll_offset = self.table_scroll_offset.saturating_sub(1);
                    return;
                }
                if self.table_cursor > 0 {
                    self.table_cursor -= 1;
                }
                self.adjust_table_scroll();
            }
        }
    }

    fn move_down(&mut self) {
        match self.active_panel {
            ActivePanel::Sidebar => {
                let visible = self.visible_tree_len();
                if self.tree_cursor + 1 < visible {
                    self.tree_cursor += 1;
                }
                self.adjust_sidebar_scroll();
            }
            ActivePanel::Center => {
                if self.active_tab == 1 {
                    // Top tab: line-by-line scroll (Paragraph, no row selection)
                    let max = self.top_tab_line_count();
                    if max > 0 && self.table_scroll_offset + 1 < max {
                        self.table_scroll_offset += 1;
                    }
                    return;
                }
                // Clamp to current row count to prevent unbounded cursor growth
                let max = self.current_row_count();
                if max > 0 && self.table_cursor + 1 < max {
                    self.table_cursor += 1;
                }
                self.adjust_table_scroll();
            }
        }
    }

    fn page_up(&mut self) {
        match self.active_panel {
            ActivePanel::Sidebar => {
                let jump = (self.sidebar_page_size / 2).max(1);
                self.tree_cursor = self.tree_cursor.saturating_sub(jump);
                self.adjust_sidebar_scroll();
            }
            ActivePanel::Center => {
                let jump = (self.page_size / 2).max(1);
                if self.active_tab == 1 {
                    self.table_scroll_offset = self.table_scroll_offset.saturating_sub(jump);
                    return;
                }
                self.table_cursor = self.table_cursor.saturating_sub(jump);
                self.adjust_table_scroll();
            }
        }
    }

    fn page_down(&mut self) {
        match self.active_panel {
            ActivePanel::Sidebar => {
                let jump = (self.sidebar_page_size / 2).max(1);
                let visible = self.visible_tree_len();
                self.tree_cursor = (self.tree_cursor + jump).min(visible.saturating_sub(1));
                self.adjust_sidebar_scroll();
            }
            ActivePanel::Center => {
                let jump = (self.page_size / 2).max(1);
                if self.active_tab == 1 {
                    let max = self.top_tab_line_count();
                    self.table_scroll_offset =
                        (self.table_scroll_offset + jump).min(max.saturating_sub(1));
                    return;
                }
                let max = self.current_row_count();
                if max > 0 {
                    self.table_cursor = (self.table_cursor + jump).min(max - 1);
                }
                self.adjust_table_scroll();
            }
        }
    }

    fn jump_home(&mut self) {
        match self.active_panel {
            ActivePanel::Sidebar => {
                self.tree_cursor = 0;
                self.adjust_sidebar_scroll();
            }
            ActivePanel::Center => {
                if self.active_tab == 1 {
                    self.table_scroll_offset = 0;
                } else {
                    self.table_cursor = 0;
                    self.adjust_table_scroll();
                }
            }
        }
    }

    fn jump_end(&mut self) {
        match self.active_panel {
            ActivePanel::Sidebar => {
                let visible = self.visible_tree_len();
                self.tree_cursor = visible.saturating_sub(1);
                self.adjust_sidebar_scroll();
            }
            ActivePanel::Center => {
                if self.active_tab == 1 {
                    let max = self.top_tab_line_count();
                    self.table_scroll_offset = max.saturating_sub(1);
                } else {
                    let max = self.current_row_count();
                    self.table_cursor = max.saturating_sub(1);
                    self.adjust_table_scroll();
                }
            }
        }
    }

    /// Adjust sidebar_scroll_offset to keep tree_cursor visible.
    /// viewport_height is set externally; default 0 means no scrolling needed.
    fn adjust_sidebar_scroll(&mut self) {
        // Will be used by render_sidebar to set viewport height
        // For now, ensure offset doesn't exceed cursor
        if self.tree_cursor < self.sidebar_scroll_offset {
            self.sidebar_scroll_offset = self.tree_cursor;
        }
        // The upper bound adjustment happens in ensure_sidebar_scroll_visible
    }

    /// Ensure sidebar cursor is visible within a given viewport height.
    /// Also clamps scroll offset so we never over-scroll past the end of content.
    pub fn ensure_sidebar_scroll_visible(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        let visible_len = self.visible_tree_len();
        // Clamp scroll offset so the last item is at the bottom, never beyond
        let max_offset = visible_len.saturating_sub(viewport_height);
        if self.sidebar_scroll_offset > max_offset {
            self.sidebar_scroll_offset = max_offset;
        }
        if self.tree_cursor < self.sidebar_scroll_offset {
            self.sidebar_scroll_offset = self.tree_cursor;
        } else if self.tree_cursor >= self.sidebar_scroll_offset + viewport_height {
            self.sidebar_scroll_offset = self.tree_cursor - viewport_height + 1;
        }
    }

    /// Adjust table_scroll_offset to keep table_cursor visible.
    fn adjust_table_scroll(&mut self) {
        if self.table_cursor < self.table_scroll_offset {
            self.table_scroll_offset = self.table_cursor;
        }
        // The upper bound adjustment happens in ensure_table_scroll_visible
    }

    /// Ensure table cursor is visible within a given viewport height.
    /// Also clamps scroll offset so we never over-scroll past the end of content.
    ///
    /// **Top tab (active_tab==1)**: uses table_scroll_offset for Paragraph scroll
    /// (no table_cursor), so we only clamp the offset against line count.
    pub fn ensure_table_scroll_visible(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }

        // Top tab: Paragraph-based scroll, no table_cursor involvement
        if self.active_tab == 1 {
            let line_count = self.top_tab_line_count();
            let max_offset = line_count.saturating_sub(viewport_height);
            if self.table_scroll_offset > max_offset {
                self.table_scroll_offset = max_offset;
            }
            return;
        }

        // Resources tab: table_cursor-based scroll
        let row_count = self.current_row_count();
        if row_count > 0 {
            let max_offset = row_count.saturating_sub(viewport_height);
            if self.table_scroll_offset > max_offset {
                self.table_scroll_offset = max_offset;
            }
        } else {
            self.table_scroll_offset = 0;
        }
        if self.table_cursor < self.table_scroll_offset {
            self.table_scroll_offset = self.table_cursor;
        } else if self.table_cursor >= self.table_scroll_offset + viewport_height {
            self.table_scroll_offset = self.table_cursor - viewport_height + 1;
        }
    }

    fn handle_enter(&mut self) {
        if self.active_panel != ActivePanel::Sidebar {
            return;
        }
        let visible = self.visible_tree_indices();
        if self.tree_cursor >= visible.len() {
            return;
        }
        let idx = visible[self.tree_cursor];
        let node = &self.tree[idx];

        match &node.node_type {
            NodeType::Root => {
                self.tree[idx].expanded = !self.tree[idx].expanded;
            }
            NodeType::Cluster(name) => {
                let name = name.clone();
                let is_expanded = self.tree[idx].expanded;
                if is_expanded {
                    // Collapse: remove children and reset loaded flag
                    self.tree[idx].expanded = false;
                    self.tree[idx].children_loaded = false;
                    self.remove_children(idx);
                    // Clear namespace selection so collapsed cluster shows marker
                    if self.selected_cluster.as_ref() == Some(&name) {
                        self.selected_namespace = None;
                    }
                } else {
                    self.tree[idx].expanded = true;
                    self.selected_cluster = Some(name.clone());
                    self.selected_namespace = None;
                    self.table_cursor = 0;
                    self.table_scroll_offset = 0;
                    self.needs_refresh = true;
                    self.fetch_generation += 1;
                    self.is_fetching = false;
                    // Immediately populate tree from cached snapshots
                    self.sync_tree_from_snapshots();
                }
            }
            NodeType::Namespace { cluster, namespace } => {
                self.selected_cluster = Some(cluster.clone());
                self.selected_namespace = if namespace == "All Namespaces" {
                    None
                } else {
                    Some(namespace.clone())
                };
                self.table_cursor = 0;
                self.table_scroll_offset = 0;
                self.needs_refresh = true;
                self.fetch_generation += 1;
                self.is_fetching = false;
            }
            NodeType::InfraHeader => {
                if self.tree[idx].expanded {
                    // Collapse: remove children and reset loaded flag
                    self.tree[idx].expanded = false;
                    self.tree[idx].children_loaded = false;
                    self.remove_children(idx);
                } else {
                    // Expand: populate infra children from cached data
                    self.tree[idx].expanded = true;
                    self.sync_infra_tree();
                }
            }
            NodeType::InfraItem(_) => {}
        }
    }

    fn collapse_node(&mut self) {
        if self.active_panel != ActivePanel::Sidebar {
            return;
        }
        let visible = self.visible_tree_indices();
        if self.tree_cursor >= visible.len() {
            return;
        }
        let idx = visible[self.tree_cursor];
        let node_type = self.tree[idx].node_type.clone();

        // Leaf nodes can't collapse — navigate to parent instead
        if matches!(
            node_type,
            NodeType::Namespace { .. } | NodeType::InfraItem(_)
        ) {
            self.navigate_to_parent(&visible, idx);
            return;
        }

        if self.tree[idx].expanded {
            self.tree[idx].expanded = false;
            self.tree[idx].children_loaded = false;
            self.remove_children(idx);
        } else {
            // Already collapsed — navigate to parent
            self.navigate_to_parent(&visible, idx);
        }
    }

    fn expand_node(&mut self) {
        if self.active_panel != ActivePanel::Sidebar {
            return;
        }
        let visible = self.visible_tree_indices();
        if self.tree_cursor >= visible.len() {
            return;
        }
        let idx = visible[self.tree_cursor];

        // Leaf nodes can't expand — no-op
        if matches!(
            self.tree[idx].node_type,
            NodeType::Namespace { .. } | NodeType::InfraItem(_)
        ) {
            return;
        }

        if !self.tree[idx].expanded {
            self.tree[idx].expanded = true;
            // Populate children from cached snapshots (cluster nodes)
            // No selection change — expand only. Use Enter to select.
            self.sync_tree_from_snapshots();
            self.sync_infra_tree();
        }
    }

    /// Navigate cursor to the parent of the node at `tree_idx`.
    /// Parent is the nearest preceding node with a smaller depth.
    fn navigate_to_parent(&mut self, visible: &[usize], tree_idx: usize) {
        let target_depth = self.tree[tree_idx].depth;
        if target_depth == 0 {
            return; // Root nodes have no parent
        }
        // Walk backwards through visible indices to find parent
        let cursor_vi = visible.iter().position(|&i| i == tree_idx).unwrap_or(0);
        for vi in (0..cursor_vi).rev() {
            if self.tree[visible[vi]].depth < target_depth {
                self.tree_cursor = vi;
                self.adjust_sidebar_scroll();
                return;
            }
        }
    }

    fn remove_children(&mut self, parent_idx: usize) {
        let parent_depth = self.tree[parent_idx].depth;
        // Count how many children to remove (O(k))
        let mut count = 0;
        while parent_idx + 1 + count < self.tree.len()
            && self.tree[parent_idx + 1 + count].depth > parent_depth
        {
            count += 1;
        }
        if count > 0 {
            // Drain the range in one O(n) operation instead of O(n*k)
            self.tree.drain((parent_idx + 1)..(parent_idx + 1 + count));
        }
        // Clamp cursor to visible range after children removal
        let visible_len = self.visible_tree_len();
        if visible_len > 0 && self.tree_cursor >= visible_len {
            self.tree_cursor = visible_len - 1;
        }
    }

    pub fn visible_tree_indices(&self) -> Vec<usize> {
        let mut result = Vec::new();
        let mut skip_depth: Option<usize> = None;

        for (i, node) in self.tree.iter().enumerate() {
            if let Some(sd) = skip_depth {
                if node.depth > sd {
                    continue;
                }
                skip_depth = None;
            }

            result.push(i);

            if !node.expanded {
                skip_depth = Some(node.depth);
            }
        }
        result
    }

    pub fn visible_tree_len(&self) -> usize {
        let mut count = 0;
        let mut skip_depth: Option<usize> = None;
        for node in &self.tree {
            if let Some(sd) = skip_depth {
                if node.depth > sd {
                    continue;
                }
                skip_depth = None;
            }
            count += 1;
            if !node.expanded {
                skip_depth = Some(node.depth);
            }
        }
        count
    }

    /// Load infrastructure data from SDI directory.
    /// If infra header is already expanded, removes stale children and re-populates.
    pub fn load_infra(&mut self) {
        let sdi_dir = std::path::Path::new("_generated/sdi");
        self.infra = infra::load_sdi_state(sdi_dir);
        // Force re-sync: reset children_loaded so sync_infra_tree re-populates
        if let Some(idx) = self
            .tree
            .iter()
            .position(|n| matches!(&n.node_type, NodeType::InfraHeader))
        {
            if self.tree[idx].children_loaded {
                self.remove_children(idx);
                self.tree[idx].children_loaded = false;
            }
        }
        self.sync_infra_tree();
    }

    /// Sync infrastructure items into the sidebar tree
    fn sync_infra_tree(&mut self) {
        let infra_idx = self
            .tree
            .iter()
            .position(|n| matches!(&n.node_type, NodeType::InfraHeader));

        if let Some(idx) = infra_idx {
            if self.tree[idx].expanded && !self.tree[idx].children_loaded {
                let depth = self.tree[idx].depth + 1;
                let mut children = Vec::new();

                for pool in &self.infra.sdi_pools {
                    let label = format!(
                        "{} ({}) — {} VMs",
                        pool.pool_name,
                        pool.purpose,
                        pool.nodes.len()
                    );
                    children.push(TreeNode {
                        label,
                        depth,
                        expanded: false,
                        node_type: NodeType::InfraItem(pool.pool_name.clone()),
                        children_loaded: false,
                    });
                }

                if children.is_empty() {
                    children.push(TreeNode {
                        label: "No SDI data".to_string(),
                        depth,
                        expanded: false,
                        node_type: NodeType::InfraItem("none".into()),
                        children_loaded: false,
                    });
                }

                // Insert children in one O(n) splice
                let insert_at = idx + 1;
                self.tree.splice(insert_at..insert_at, children);
                self.tree[idx].children_loaded = true;
            }
        }
    }

    /// Populate namespace children for expanded clusters from snapshot data.
    /// Detects namespace list changes and re-populates if needed.
    pub fn sync_tree_from_snapshots(&mut self) {
        // Collect snapshot data first to avoid borrow conflict
        let snapshot_data: Vec<(String, Vec<String>)> = self
            .snapshots
            .iter()
            .map(|s| (s.name.clone(), s.namespaces.clone()))
            .collect();

        for (snap_name, snap_namespaces) in &snapshot_data {
            // Find the cluster node
            let cluster_idx = self
                .tree
                .iter()
                .position(|n| matches!(&n.node_type, NodeType::Cluster(name) if name == snap_name));

            if let Some(idx) = cluster_idx {
                if !self.tree[idx].expanded {
                    continue;
                }

                // Check if namespace list changed (stale sidebar refresh)
                if self.tree[idx].children_loaded {
                    let existing_ns: Vec<String> = self.tree[(idx + 1)..]
                        .iter()
                        .take_while(|n| n.depth > self.tree[idx].depth)
                        .filter_map(|n| match &n.node_type {
                            NodeType::Namespace { namespace, .. }
                                if namespace != "All Namespaces" =>
                            {
                                Some(namespace.clone())
                            }
                            _ => None,
                        })
                        .collect();

                    if existing_ns == *snap_namespaces {
                        continue; // namespaces unchanged, skip
                    }
                    // Namespaces changed — remove old children and re-populate
                    self.remove_children(idx);
                    self.tree[idx].children_loaded = false;
                }

                let depth = self.tree[idx].depth + 1;
                let cluster_name = snap_name.clone();

                let mut children = vec![TreeNode {
                    label: "All Namespaces".to_string(),
                    depth,
                    expanded: false,
                    node_type: NodeType::Namespace {
                        cluster: cluster_name.clone(),
                        namespace: "All Namespaces".to_string(),
                    },
                    children_loaded: false,
                }];

                for ns in snap_namespaces {
                    children.push(TreeNode {
                        label: ns.clone(),
                        depth,
                        expanded: false,
                        node_type: NodeType::Namespace {
                            cluster: cluster_name.clone(),
                            namespace: ns.clone(),
                        },
                        children_loaded: false,
                    });
                }

                // Insert children after cluster node in one O(n) splice
                let insert_at = idx + 1;
                self.tree.splice(insert_at..insert_at, children);

                self.tree[idx].children_loaded = true;
            }
        }
    }

    /// Check if a resource view is showing stale (cached) data from a previous fetch cycle
    pub fn is_view_stale(&self, view: ResourceView) -> bool {
        match self.last_fetched_resource {
            None => false, // full fetch or no fetch yet — not stale
            Some(active) => active != view.to_active_resource(),
        }
    }

    /// Number of content lines in the Top tab (for scroll clamping).
    fn top_tab_line_count(&self) -> usize {
        match self.current_snapshot() {
            Some(snap) => 2 + snap.nodes.len().max(1), // header + blank + nodes (or "No data")
            None => 1,
        }
    }

    /// Approximate content line count for the help overlay.
    /// Mirrors the line-building logic in render_help_overlay (ui.rs).
    /// Used to cap help_scroll_offset in the event handler.
    fn help_content_line_count(&self) -> u16 {
        let context_lines: u16 = if self.search_active {
            6 // section + blank + 4 keys (<type>, Enter, ESC, Backspace)
        } else {
            match self.active_panel {
                ActivePanel::Sidebar => 7, // section + blank + 5 keys (j/k, PgUp/Dn, Home/End, h/l, Enter)
                ActivePanel::Center => {
                    if self.active_tab == 1 {
                        5 // section + blank + 3 keys (j/k, PgUp/Dn, Home/End)
                    } else {
                        7 // section + blank + 4 keys (j/k, PgUp/Dn, Home/End, pdscn) + desc line
                    }
                }
            }
        };
        // Global section: blank + section + blank + 8 keys + blank + footer = 13
        context_lines + 13
    }

    /// Maximum scroll offset for help overlay, accounting for viewport height (US-204).
    /// Prevents offset from growing beyond what the render actually uses.
    fn help_max_scroll(&self) -> u16 {
        self.help_content_line_count()
            .saturating_sub(self.help_viewport_height)
    }

    /// Get the number of rows in the current resource view (for cursor clamping).
    /// Respects active search filter so cursor stays within visible bounds.
    pub fn current_row_count(&self) -> usize {
        match self.current_snapshot() {
            Some(snap) => match self.resource_view {
                ResourceView::Pods => snap
                    .pods
                    .iter()
                    .filter(|p| self.matches_search_with_ns(&p.name, &p.namespace))
                    .count(),
                ResourceView::Deployments => snap
                    .deployments
                    .iter()
                    .filter(|d| self.matches_search_with_ns(&d.name, &d.namespace))
                    .count(),
                ResourceView::Services => snap
                    .services
                    .iter()
                    .filter(|s| self.matches_search_with_ns(&s.name, &s.namespace))
                    .count(),
                ResourceView::ConfigMaps => snap
                    .configmaps
                    .iter()
                    .filter(|c| self.matches_search_with_ns(&c.name, &c.namespace))
                    .count(),
                ResourceView::Nodes => snap
                    .nodes
                    .iter()
                    .filter(|n| self.matches_search(&n.name))
                    .count(),
            },
            None => 0,
        }
    }

    pub fn current_snapshot(&self) -> Option<&ClusterSnapshot> {
        self.selected_cluster
            .as_ref()
            .and_then(|name| self.snapshots.iter().find(|s| &s.name == name))
    }

    /// Check if all discovered clusters have failed to connect
    pub fn all_clusters_failed(&self) -> bool {
        !self.cluster_connection_status.is_empty()
            && self.clusters.is_empty()
            && self
                .cluster_connection_status
                .values()
                .all(|s| matches!(s, ConnectionStatus::Failed(_)))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a minimal App with two clusters for testing navigation
    fn test_app() -> App {
        // Simulate two clusters without real kube clients
        let mut app = App {
            running: true,
            active_panel: ActivePanel::Sidebar,
            active_tab: 0,
            tabs: vec![
                Tab {
                    name: "Resources".into(),
                    closable: false,
                },
                Tab {
                    name: "Top".into(),
                    closable: false,
                },
            ],
            resource_view: ResourceView::Pods,
            tree: vec![
                TreeNode {
                    label: "ScaleX".into(),
                    depth: 0,
                    expanded: true,
                    node_type: NodeType::Root,
                    children_loaded: true,
                },
                TreeNode {
                    label: "tower".into(),
                    depth: 1,
                    expanded: false,
                    node_type: NodeType::Cluster("tower".into()),
                    children_loaded: false,
                },
                TreeNode {
                    label: "sandbox".into(),
                    depth: 1,
                    expanded: false,
                    node_type: NodeType::Cluster("sandbox".into()),
                    children_loaded: false,
                },
                TreeNode {
                    label: "Infrastructure".into(),
                    depth: 0,
                    expanded: false,
                    node_type: NodeType::InfraHeader,
                    children_loaded: false,
                },
            ],
            tree_cursor: 0,
            sidebar_scroll_offset: 0,
            table_cursor: 0,
            table_scroll_offset: 0,
            selected_cluster: None,
            selected_namespace: None,
            clusters: vec![],
            snapshots: vec![],
            infra: InfraSnapshot::default(),
            api_latency_ms: 0,
            show_help: false,
            help_scroll_offset: 0,
            search_active: false,
            search_query: None,
            self_rss_mb: None,
            refresh_secs: 1,
            needs_refresh: false,
            cluster_connection_status: HashMap::new(),
            discover_complete: true,
            tunnel_pids: Vec::new(),
            is_fetching: false,
            fetch_started_at: None,
            tick_count: 0,
            last_fetched_resource: None,
            fetch_generation: 0,
            fetch_timed_out: false,
            page_size: 0,
            sidebar_page_size: 0,
            help_viewport_height: 0,
        };
        // Move cursor to first cluster (tower)
        app.tree_cursor = 1;
        app
    }

    #[test]
    fn move_up_does_not_change_selection() {
        let mut app = test_app();
        app.selected_cluster = Some("tower".into());
        app.tree_cursor = 2; // on sandbox

        app.handle_event(AppEvent::Up);

        assert_eq!(app.tree_cursor, 1); // moved to tower
        assert_eq!(app.selected_cluster, Some("tower".into())); // unchanged
    }

    #[test]
    fn move_down_does_not_change_selection() {
        let mut app = test_app();
        app.selected_cluster = Some("tower".into());
        app.tree_cursor = 1; // on tower

        app.handle_event(AppEvent::Down);

        assert_eq!(app.tree_cursor, 2); // moved to sandbox
        assert_eq!(app.selected_cluster, Some("tower".into())); // unchanged
    }

    #[test]
    fn expand_node_does_not_set_selection() {
        let mut app = test_app();
        app.tree_cursor = 1; // on tower cluster
        assert!(app.selected_cluster.is_none());

        app.handle_event(AppEvent::Right); // expand tower

        assert!(app.tree[1].expanded); // tree expanded
        assert!(app.selected_cluster.is_none()); // selection unchanged
    }

    #[test]
    fn collapse_node_does_not_change_selection() {
        let mut app = test_app();
        app.tree[1].expanded = true; // tower expanded
        app.selected_cluster = Some("tower".into());
        app.tree_cursor = 1;

        app.handle_event(AppEvent::Left); // collapse tower

        assert!(!app.tree[1].expanded);
        assert_eq!(app.selected_cluster, Some("tower".into())); // unchanged
    }

    #[test]
    fn enter_on_cluster_sets_selection() {
        let mut app = test_app();
        app.tree_cursor = 2; // on sandbox
        assert!(app.selected_cluster.is_none());

        app.handle_event(AppEvent::Enter);

        assert_eq!(app.selected_cluster, Some("sandbox".into()));
    }

    #[test]
    fn enter_on_namespace_sets_selection() {
        let mut app = test_app();
        // Simulate expanded tower with namespace children
        app.tree[1].expanded = true;
        app.tree[1].children_loaded = true;
        app.tree.insert(
            2,
            TreeNode {
                label: "kube-system".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "kube-system".into(),
                },
                children_loaded: false,
            },
        );
        app.tree_cursor = 2; // on kube-system namespace

        app.handle_event(AppEvent::Enter);

        assert_eq!(app.selected_cluster, Some("tower".into()));
        assert_eq!(app.selected_namespace, Some("kube-system".into()));
    }

    #[test]
    fn view_switch_sets_needs_refresh() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.resource_view = ResourceView::Pods;
        assert!(!app.needs_refresh);

        app.handle_event(AppEvent::ResourceType('d')); // switch to Deployments

        assert_eq!(app.resource_view, ResourceView::Deployments);
        assert!(app.needs_refresh);
    }

    #[test]
    fn same_view_does_not_trigger_refresh() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.resource_view = ResourceView::Pods;

        app.handle_event(AppEvent::ResourceType('p')); // same view

        assert!(!app.needs_refresh);
    }

    #[test]
    fn resource_shortcut_ignored_from_sidebar() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Sidebar;
        app.resource_view = ResourceView::Pods;

        app.handle_event(AppEvent::ResourceType('d'));

        // Resource shortcuts only work in center panel
        assert_eq!(app.resource_view, ResourceView::Pods);
        assert!(!app.needs_refresh);
    }

    // --- US-030: Table cursor bounds clamping ---

    #[test]
    fn clamp_table_cursor_within_bounds() {
        let mut app = test_app();
        app.table_cursor = 3;
        app.clamp_table_cursor(5);
        assert_eq!(app.table_cursor, 3); // no change
    }

    #[test]
    fn clamp_table_cursor_at_boundary() {
        let mut app = test_app();
        app.table_cursor = 5;
        app.clamp_table_cursor(5);
        assert_eq!(app.table_cursor, 4); // clamped to last index
    }

    #[test]
    fn clamp_table_cursor_far_exceeds() {
        let mut app = test_app();
        app.table_cursor = 100;
        app.clamp_table_cursor(5);
        assert_eq!(app.table_cursor, 4);
    }

    #[test]
    fn clamp_table_cursor_empty_list() {
        let mut app = test_app();
        app.table_cursor = 5;
        app.clamp_table_cursor(0);
        assert_eq!(app.table_cursor, 0);
    }

    #[test]
    fn move_down_center_increments() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.selected_cluster = Some("tower".into());
        // Add snapshot with 5 pods so move_down has room
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec![],
            nodes: vec![],
            pods: (0..5)
                .map(|i| crate::dash::data::PodInfo {
                    name: format!("pod-{}", i),
                    namespace: "default".into(),
                    status: "Running".into(),
                    ready: "1/1".into(),
                    restarts: 0,
                    age: "1h".into(),
                    node: "n1".into(),
                })
                .collect(),
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });
        app.table_cursor = 0;
        app.handle_event(AppEvent::Down);
        assert_eq!(app.table_cursor, 1);
    }

    // --- US-031: Search input resets table cursor ---

    #[test]
    fn search_up_resets_cursor() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.table_cursor = 5;

        app.handle_event(AppEvent::Up); // appends 'k'

        assert_eq!(app.search_query, Some("k".to_string()));
        assert_eq!(app.table_cursor, 0);
    }

    #[test]
    fn search_down_resets_cursor() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.table_cursor = 5;

        app.handle_event(AppEvent::Down); // appends 'j'

        assert_eq!(app.search_query, Some("j".to_string()));
        assert_eq!(app.table_cursor, 0);
    }

    #[test]
    fn search_refresh_resets_cursor() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.table_cursor = 5;

        app.handle_event(AppEvent::Refresh); // appends 'r'

        assert_eq!(app.search_query, Some("r".to_string()));
        assert_eq!(app.table_cursor, 0);
    }

    #[test]
    fn search_slash_resets_cursor() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.table_cursor = 5;

        app.handle_event(AppEvent::Search); // appends '/'

        assert_eq!(app.search_query, Some("/".to_string()));
        assert_eq!(app.table_cursor, 0);
    }

    // --- US-032: Tree collapse adjusts cursor ---

    #[test]
    fn collapse_clamps_cursor_when_on_child() {
        let mut app = test_app();
        // Expand tower with namespace children
        app.tree[1].expanded = true;
        app.tree[1].children_loaded = true;
        app.tree.insert(
            2,
            TreeNode {
                label: "All Namespaces".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "All Namespaces".into(),
                },
                children_loaded: false,
            },
        );
        app.tree.insert(
            3,
            TreeNode {
                label: "kube-system".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "kube-system".into(),
                },
                children_loaded: false,
            },
        );
        app.tree.insert(
            4,
            TreeNode {
                label: "default".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "default".into(),
                },
                children_loaded: false,
            },
        );
        // tree: ScaleX, tower(exp), AllNS, kube-system, default, sandbox, Infra
        // visible indices: 0,1,2,3,4,5,6 (7 items)
        app.tree_cursor = 4; // on "default" child

        app.handle_event(AppEvent::Left); // collapse — cursor on child, should collapse parent? No — Left on child collapses child (which is a leaf, no-op)
                                          // Actually Left on a non-expandable node is a no-op. Let's test via handle_enter on the cluster node.
                                          // Reset: put cursor on tower (index 1) and collapse
        app.tree_cursor = 1;
        app.handle_event(AppEvent::Left); // collapse tower

        assert!(!app.tree[1].expanded);
        // After removing 3 children, tree: ScaleX, tower, sandbox, Infra (4 visible)
        assert!(app.tree_cursor < app.visible_tree_len());
    }

    #[test]
    fn collapse_with_cursor_beyond_visible_clamps() {
        let mut app = test_app();
        app.tree[1].expanded = true;
        app.tree[1].children_loaded = true;
        // Add 3 children
        for i in 0..3 {
            app.tree.insert(
                2 + i,
                TreeNode {
                    label: format!("ns-{}", i),
                    depth: 2,
                    expanded: false,
                    node_type: NodeType::Namespace {
                        cluster: "tower".into(),
                        namespace: format!("ns-{}", i),
                    },
                    children_loaded: false,
                },
            );
        }
        // 7 visible items: ScaleX, tower, ns-0, ns-1, ns-2, sandbox, Infra
        app.tree_cursor = 6; // on Infrastructure (last visible)

        // Now collapse tower from code (simulating programmatic collapse)
        app.tree[1].expanded = false;
        app.remove_children(1);

        // After: ScaleX, tower, sandbox, Infra (4 visible)
        assert!(app.tree_cursor < app.visible_tree_len());
        assert_eq!(app.tree_cursor, 3); // clamped from 6 to 3 (last valid)
    }

    // --- US-033: Efficient removal with drain ---

    #[test]
    fn remove_children_drains_correctly() {
        let mut app = test_app();
        app.tree[1].expanded = true;
        app.tree[1].children_loaded = true;
        // Add children at depth 2
        app.tree.insert(
            2,
            TreeNode {
                label: "child1".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "child1".into(),
                },
                children_loaded: false,
            },
        );
        app.tree.insert(
            3,
            TreeNode {
                label: "child2".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "child2".into(),
                },
                children_loaded: false,
            },
        );
        // tree: ScaleX(0), tower(1), child1(2), child2(3), sandbox(4), Infra(5)
        assert_eq!(app.tree.len(), 6);

        app.remove_children(1);

        // tree: ScaleX(0), tower(1), sandbox(2), Infra(3)
        assert_eq!(app.tree.len(), 4);
        assert_eq!(app.tree[2].label, "sandbox");
    }

    // --- US-034: Sidebar scroll offset ---

    #[test]
    fn ensure_sidebar_scroll_cursor_below_viewport() {
        let mut app = test_app();
        app.tree_cursor = 15;
        app.sidebar_scroll_offset = 0;

        app.ensure_sidebar_scroll_visible(10);

        assert!(app.sidebar_scroll_offset >= 6); // 15 - 10 + 1 = 6
    }

    #[test]
    fn ensure_sidebar_scroll_cursor_above_viewport() {
        let mut app = test_app();
        app.tree_cursor = 2;
        app.sidebar_scroll_offset = 5;

        app.ensure_sidebar_scroll_visible(10);

        // Content (4 items) fits in viewport (10), so offset clamped to 0
        assert_eq!(app.sidebar_scroll_offset, 0);
    }

    #[test]
    fn ensure_sidebar_scroll_overscroll_clamped() {
        let mut app = test_app();
        // Add enough items so content exceeds viewport
        for i in 0..20 {
            app.tree.insert(
                1,
                TreeNode {
                    label: format!("extra-{}", i),
                    depth: 1,
                    expanded: false,
                    node_type: NodeType::Cluster(format!("extra-{}", i)),
                    children_loaded: false,
                },
            );
        }
        // 24 visible items, viewport = 10, max_offset = 14
        app.tree_cursor = 5;
        app.sidebar_scroll_offset = 20; // way past end

        app.ensure_sidebar_scroll_visible(10);

        assert!(app.sidebar_scroll_offset <= 14); // clamped to max
    }

    // --- US-035: Table scroll offset ---

    #[test]
    fn ensure_table_scroll_cursor_below_viewport() {
        let mut app = test_app();
        app.table_cursor = 20;
        app.table_scroll_offset = 0;

        app.ensure_table_scroll_visible(10);

        assert_eq!(app.table_scroll_offset, 11); // 20 - 10 + 1
    }

    #[test]
    fn ensure_table_scroll_cursor_above_viewport() {
        let mut app = test_app();
        app.table_cursor = 3;
        app.table_scroll_offset = 8;

        app.ensure_table_scroll_visible(10);

        // No data (row_count=0), so offset clamped to 0
        assert_eq!(app.table_scroll_offset, 0);
    }

    #[test]
    fn ensure_table_scroll_overscroll_clamped() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.selected_cluster = Some("tower".into());
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec![],
            nodes: vec![],
            pods: (0..20)
                .map(|i| crate::dash::data::PodInfo {
                    name: format!("pod-{}", i),
                    namespace: "default".into(),
                    status: "Running".into(),
                    ready: "1/1".into(),
                    restarts: 0,
                    age: "1h".into(),
                    node: "n1".into(),
                })
                .collect(),
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });
        // 20 rows, viewport = 10, max_offset = 10
        app.table_cursor = 5;
        app.table_scroll_offset = 15; // way past end

        app.ensure_table_scroll_visible(10);

        assert!(app.table_scroll_offset <= 10); // clamped
    }

    // --- US-040: Refresh key triggers data refresh ---

    #[test]
    fn refresh_key_sets_needs_refresh() {
        let mut app = test_app();
        app.needs_refresh = false;
        let gen_before = app.fetch_generation;

        app.handle_event(AppEvent::Refresh);

        assert!(app.needs_refresh);
        assert_eq!(app.fetch_generation, gen_before + 1);
    }

    // --- US-041: table_scroll_offset reset on context changes ---

    #[test]
    fn resource_view_switch_resets_scroll_offset() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.table_scroll_offset = 10;
        app.resource_view = ResourceView::Pods;

        app.handle_event(AppEvent::ResourceType('d')); // switch to Deployments

        assert_eq!(app.table_scroll_offset, 0);
        assert_eq!(app.table_cursor, 0);
    }

    #[test]
    fn search_cancel_resets_scroll_offset() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("test".into());
        app.table_scroll_offset = 15;
        app.table_cursor = 5;

        app.handle_event(AppEvent::Escape);

        assert_eq!(app.table_scroll_offset, 0);
        assert_eq!(app.table_cursor, 0);
        assert!(!app.search_active);
    }

    #[test]
    fn search_start_resets_scroll_offset() {
        let mut app = test_app();
        app.table_scroll_offset = 10;
        app.table_cursor = 8;

        app.handle_event(AppEvent::Search);

        assert_eq!(app.table_scroll_offset, 0);
        assert_eq!(app.table_cursor, 0);
        assert!(app.search_active);
    }

    // --- US-042: expand_node populates children from cache ---

    #[test]
    fn expand_node_populates_children_from_cached_snapshot() {
        let mut app = test_app();
        // Add a snapshot with namespaces
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec!["default".into(), "kube-system".into()],
            nodes: vec![],
            pods: vec![],
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });

        // Cursor on tower cluster (index 1 in visible tree)
        app.active_panel = ActivePanel::Sidebar;
        app.tree_cursor = 1;
        assert!(!app.tree[1].expanded);

        app.handle_event(AppEvent::Right); // expand via Right arrow

        assert!(app.tree[1].expanded);
        assert!(app.tree[1].children_loaded);
        // Should have "All Namespaces", "default", "kube-system" as children
        assert_eq!(app.tree.len(), 4 + 3); // original 4 + 3 namespace children
    }

    // --- US-043: move_down bounded by row count ---

    #[test]
    fn move_down_center_no_data_stays_at_zero() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.table_cursor = 0;
        // No snapshots → current_row_count() == 0
        app.handle_event(AppEvent::Down);
        assert_eq!(app.table_cursor, 0);
    }

    // --- US-050: Search mode captures all literal characters ---

    #[test]
    fn search_quit_key_types_q() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.handle_event(AppEvent::Quit); // 'q' key
        assert!(app.search_active); // still in search
        assert_eq!(app.search_query, Some("q".to_string()));
    }

    #[test]
    fn search_help_key_types_question_mark() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.handle_event(AppEvent::Help); // '?' key
        assert!(app.search_active);
        assert_eq!(app.search_query, Some("?".to_string()));
    }

    #[test]
    fn search_left_key_types_h() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.handle_event(AppEvent::Left); // 'h' key
        assert_eq!(app.search_query, Some("h".to_string()));
    }

    #[test]
    fn search_right_key_types_l() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.handle_event(AppEvent::Right); // 'l' key
        assert_eq!(app.search_query, Some("l".to_string()));
    }

    #[test]
    fn search_arrow_keys_are_noop() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("abc".to_string());
        app.table_cursor = 3;
        for evt in [
            AppEvent::ArrowUp,
            AppEvent::ArrowDown,
            AppEvent::ArrowLeft,
            AppEvent::ArrowRight,
        ] {
            app.handle_event(evt);
            assert_eq!(app.search_query, Some("abc".to_string()));
            assert_eq!(app.table_cursor, 3);
        }
    }

    #[test]
    fn search_backspace_deletes_char() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("hello".to_string());
        app.handle_event(AppEvent::Backspace);
        assert_eq!(app.search_query, Some("hell".to_string()));
    }

    #[test]
    fn search_backspace_clears_to_none() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("x".to_string());
        app.handle_event(AppEvent::Backspace);
        assert_eq!(app.search_query, None);
    }

    #[test]
    fn search_escape_cancels() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("test".to_string());
        app.handle_event(AppEvent::Escape);
        assert!(!app.search_active);
        assert_eq!(app.search_query, None);
    }

    // --- US-051: current_row_count respects search filter ---

    #[test]
    fn move_down_clamped_by_search_filter() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.selected_cluster = Some("tower".into());
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec![],
            nodes: vec![],
            pods: vec![
                crate::dash::data::PodInfo {
                    name: "alpha-pod".into(),
                    namespace: "default".into(),
                    status: "Running".into(),
                    ready: "1/1".into(),
                    restarts: 0,
                    age: "1h".into(),
                    node: "n1".into(),
                },
                crate::dash::data::PodInfo {
                    name: "beta-pod".into(),
                    namespace: "default".into(),
                    status: "Running".into(),
                    ready: "1/1".into(),
                    restarts: 0,
                    age: "1h".into(),
                    node: "n1".into(),
                },
            ],
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });
        // Search for "alpha" → only 1 result
        app.search_query = Some("alpha".into());
        app.table_cursor = 0;
        app.handle_event(AppEvent::Down);
        // Should not exceed filtered count (1 item → cursor stays at 0)
        assert_eq!(app.table_cursor, 0);
    }

    // --- US-052: Cluster selection resets table cursor ---

    #[test]
    fn enter_on_cluster_resets_table_cursor() {
        let mut app = test_app();
        app.table_cursor = 10;
        app.table_scroll_offset = 5;
        app.tree_cursor = 2; // sandbox
        app.handle_event(AppEvent::Enter);
        assert_eq!(app.selected_cluster, Some("sandbox".into()));
        assert_eq!(app.table_cursor, 0);
        assert_eq!(app.table_scroll_offset, 0);
    }

    // --- US-050 (v5): CharInput in search mode ---

    #[test]
    fn search_char_input_appends_to_query() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());

        app.handle_event(AppEvent::CharInput('a'));
        app.handle_event(AppEvent::CharInput('b'));
        app.handle_event(AppEvent::CharInput('c'));

        assert_eq!(app.search_query, Some("abc".to_string()));
        assert!(app.search_active);
    }

    #[test]
    fn search_char_input_resets_table_cursor() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.table_cursor = 5;

        app.handle_event(AppEvent::CharInput('x'));

        assert_eq!(app.table_cursor, 0);
    }

    #[test]
    fn char_input_ignored_outside_search() {
        let mut app = test_app();
        app.search_active = false;

        app.handle_event(AppEvent::CharInput('a'));

        // Should not change any state
        assert!(app.running);
        assert!(!app.search_active);
    }

    // --- US-051 (v5): ForceQuit always quits ---

    #[test]
    fn force_quit_in_search_mode() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("test".into());

        app.handle_event(AppEvent::ForceQuit);

        assert!(!app.running); // app should quit
    }

    #[test]
    fn force_quit_in_help_mode() {
        let mut app = test_app();
        app.show_help = true;

        app.handle_event(AppEvent::ForceQuit);

        assert!(!app.running); // app should quit
    }

    #[test]
    fn force_quit_in_normal_mode() {
        let mut app = test_app();

        app.handle_event(AppEvent::ForceQuit);

        assert!(!app.running);
    }

    #[test]
    fn quit_in_search_types_q_not_exit() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());

        app.handle_event(AppEvent::Quit);

        assert!(app.running); // should NOT quit
        assert_eq!(app.search_query, Some("q".to_string()));
    }

    // --- US-101: Stale namespace sidebar refresh ---

    #[test]
    fn sync_tree_updates_when_namespaces_change() {
        let mut app = test_app();
        // Initial snapshot with 2 namespaces
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec!["default".into(), "kube-system".into()],
            nodes: vec![],
            pods: vec![],
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });

        // Expand tower
        app.tree[1].expanded = true;
        app.sync_tree_from_snapshots();
        assert!(app.tree[1].children_loaded);
        // "All Namespaces" + "default" + "kube-system" = 3 children
        assert_eq!(app.tree.len(), 4 + 3); // 4 original + 3 children

        // Now namespace list changes (new namespace added)
        app.snapshots[0].namespaces =
            vec!["default".into(), "kube-system".into(), "monitoring".into()];
        app.sync_tree_from_snapshots();

        // Should now have 4 children: All NS + default + kube-system + monitoring
        assert_eq!(app.tree.len(), 4 + 4);
    }

    #[test]
    fn sync_tree_no_change_when_namespaces_same() {
        let mut app = test_app();
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec!["default".into()],
            nodes: vec![],
            pods: vec![],
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });

        app.tree[1].expanded = true;
        app.sync_tree_from_snapshots();
        let tree_len_after_first = app.tree.len();

        // Sync again with same namespaces — should not change
        app.sync_tree_from_snapshots();
        assert_eq!(app.tree.len(), tree_len_after_first);
    }

    // --- US-104/105: Scroll offset clamping ---

    #[test]
    fn table_scroll_offset_clamped_on_data_shrink() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.selected_cluster = Some("tower".into());
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec![],
            nodes: vec![],
            pods: (0..3)
                .map(|i| crate::dash::data::PodInfo {
                    name: format!("pod-{}", i),
                    namespace: "default".into(),
                    status: "Running".into(),
                    ready: "1/1".into(),
                    restarts: 0,
                    age: "1h".into(),
                    node: "n1".into(),
                })
                .collect(),
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });
        // Set scroll offset beyond data
        app.table_scroll_offset = 10;
        app.table_cursor = 2;

        app.ensure_table_scroll_visible(10);

        // 3 rows, viewport 10 → max offset = 0
        assert_eq!(app.table_scroll_offset, 0);
    }

    // --- US-201: ESC clears search filter in normal mode ---

    #[test]
    fn esc_clears_filter_in_normal_mode() {
        let mut app = test_app();
        app.search_active = false;
        app.search_query = Some("kube".to_string());
        app.table_cursor = 3;
        app.table_scroll_offset = 2;

        app.handle_event(AppEvent::Escape);

        assert!(app.search_query.is_none());
        assert_eq!(app.table_cursor, 0);
        assert_eq!(app.table_scroll_offset, 0);
    }

    #[test]
    fn esc_noop_when_no_filter() {
        let mut app = test_app();
        app.search_active = false;
        app.search_query = None;

        app.handle_event(AppEvent::Escape);

        assert!(app.search_query.is_none());
    }

    #[test]
    fn esc_noop_when_empty_filter() {
        let mut app = test_app();
        app.search_active = false;
        app.search_query = Some(String::new());

        app.handle_event(AppEvent::Escape);

        // Empty query not cleared (no filter active)
        assert_eq!(app.search_query, Some(String::new()));
    }

    // --- US-202: Sidebar selection ambiguity ---

    #[test]
    fn all_namespaces_marker_not_on_cluster() {
        let mut app = test_app();
        // Simulate: tower expanded, "All Namespaces" selected
        app.tree[1].expanded = true;
        app.tree[1].children_loaded = true;
        app.tree.insert(
            2,
            TreeNode {
                label: "All Namespaces".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "All Namespaces".into(),
                },
                children_loaded: false,
            },
        );
        app.selected_cluster = Some("tower".into());
        app.selected_namespace = None; // "All Namespaces"

        // Cluster node (tower) should NOT be active selection when it has expanded children
        let tower_node = &app.tree[1];
        let tower_is_active = match &tower_node.node_type {
            NodeType::Cluster(name) => {
                app.selected_cluster.as_ref() == Some(name)
                    && app.selected_namespace.is_none()
                    && !tower_node.expanded // <-- new condition
            }
            _ => false,
        };
        assert!(
            !tower_is_active,
            "cluster node should not show marker when expanded with All NS"
        );

        // All Namespaces node SHOULD be active selection
        let all_ns_node = &app.tree[2];
        let all_ns_is_active = match &all_ns_node.node_type {
            NodeType::Namespace { cluster, namespace } => {
                app.selected_cluster.as_ref() == Some(cluster)
                    && namespace == "All Namespaces"
                    && app.selected_namespace.is_none()
            }
            _ => false,
        };
        assert!(all_ns_is_active, "All Namespaces node should show marker");
    }

    // --- US-204: Backspace in normal mode ---

    #[test]
    fn backspace_normal_mode_noop() {
        let mut app = test_app();
        app.tree[1].expanded = true;
        app.tree_cursor = 1;
        let expanded_before = app.tree[1].expanded;

        app.handle_event(AppEvent::Backspace);

        assert_eq!(
            app.tree[1].expanded, expanded_before,
            "Backspace should not collapse node"
        );
    }

    // --- US-070: Tab key exits search and switches panel ---

    #[test]
    fn search_tab_exits_and_switches_panel() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("test".into());
        app.active_panel = ActivePanel::Center;

        app.handle_event(AppEvent::NextPanel);

        assert!(!app.search_active);
        assert_eq!(app.active_panel, ActivePanel::Sidebar);
    }

    #[test]
    fn search_shift_tab_exits_and_switches_panel() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("test".into());
        app.active_panel = ActivePanel::Center;

        app.handle_event(AppEvent::PrevPanel);

        assert!(!app.search_active);
        assert_eq!(app.active_panel, ActivePanel::Sidebar);
    }

    // --- US-073: Search input resets table_scroll_offset ---

    #[test]
    fn search_backspace_resets_scroll_offset() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("hello".into());
        app.table_scroll_offset = 10;
        app.table_cursor = 5;

        app.handle_event(AppEvent::Backspace);

        assert_eq!(app.table_cursor, 0);
        assert_eq!(app.table_scroll_offset, 0);
    }

    #[test]
    fn search_char_input_resets_scroll_offset() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());
        app.table_scroll_offset = 10;
        app.table_cursor = 5;

        app.handle_event(AppEvent::CharInput('x'));

        assert_eq!(app.table_cursor, 0);
        assert_eq!(app.table_scroll_offset, 0);
        assert_eq!(app.search_query, Some("x".into()));
    }

    // --- US-076: Empty search submit clears query ---

    #[test]
    fn search_enter_on_empty_clears_query() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some(String::new());

        app.handle_event(AppEvent::Enter);

        assert!(!app.search_active);
        assert_eq!(app.search_query, None); // cleaned up, not Some("")
    }

    #[test]
    fn search_enter_on_nonempty_keeps_filter() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("pod".into());

        app.handle_event(AppEvent::Enter);

        assert!(!app.search_active);
        assert_eq!(app.search_query, Some("pod".into()));
    }

    // --- US-078: Tab switch resets cursor ---

    #[test]
    fn tab_switch_resets_table_cursor() {
        let mut app = test_app();
        app.active_tab = 0;
        app.table_cursor = 10;
        app.table_scroll_offset = 5;

        app.handle_event(AppEvent::Tab(2)); // switch to Top tab

        assert_eq!(app.active_tab, 1);
        assert_eq!(app.table_cursor, 0);
        assert_eq!(app.table_scroll_offset, 0);
    }

    #[test]
    fn same_tab_does_not_reset_cursor() {
        let mut app = test_app();
        app.active_tab = 0;
        app.table_cursor = 10;

        app.handle_event(AppEvent::Tab(1)); // same tab

        assert_eq!(app.active_tab, 0);
        assert_eq!(app.table_cursor, 10); // unchanged
    }

    // --- US-079: all_clusters_failed helper ---

    #[test]
    fn all_clusters_failed_returns_true_when_all_fail() {
        let mut app = test_app();
        app.cluster_connection_status
            .insert("tower".into(), ConnectionStatus::Failed("timeout".into()));
        app.cluster_connection_status
            .insert("sandbox".into(), ConnectionStatus::Failed("refused".into()));
        // No clusters connected (app.clusters is empty from test_app)
        assert!(app.all_clusters_failed());
    }

    #[test]
    fn all_clusters_failed_returns_false_when_some_connected() {
        let mut app = test_app();
        app.cluster_connection_status
            .insert("tower".into(), ConnectionStatus::Connected);
        app.cluster_connection_status
            .insert("sandbox".into(), ConnectionStatus::Failed("refused".into()));
        assert!(!app.all_clusters_failed());
    }

    #[test]
    fn all_clusters_failed_returns_false_when_empty() {
        let app = test_app();
        assert!(!app.all_clusters_failed());
    }

    #[test]
    fn all_clusters_failed_returns_false_during_discovery() {
        let mut app = test_app();
        app.cluster_connection_status
            .insert("tower".into(), ConnectionStatus::Discovering);
        assert!(!app.all_clusters_failed());
    }

    // --- US-080: Infra tree refresh on load_infra ---

    #[test]
    fn load_infra_refreshes_expanded_children() {
        let mut app = test_app();
        // Expand infra header and populate with initial data
        let infra_idx = app
            .tree
            .iter()
            .position(|n| matches!(&n.node_type, NodeType::InfraHeader))
            .unwrap();
        app.tree[infra_idx].expanded = true;

        // First load — adds children
        app.infra = InfraSnapshot {
            sdi_pools: vec![infra::SdiPoolInfo {
                pool_name: "pool-A".into(),
                purpose: "test".into(),
                nodes: vec![],
            }],
            total_vms: 0,
            running_vms: 0,
        };
        app.sync_infra_tree();
        assert!(app.tree[infra_idx].children_loaded);
        let child_count_before = app.tree[(infra_idx + 1)..]
            .iter()
            .take_while(|n| n.depth > app.tree[infra_idx].depth)
            .count();
        assert_eq!(child_count_before, 1);

        // Second load via load_infra — should update children even though already loaded
        app.infra = InfraSnapshot {
            sdi_pools: vec![
                infra::SdiPoolInfo {
                    pool_name: "pool-B".into(),
                    purpose: "new".into(),
                    nodes: vec![],
                },
                infra::SdiPoolInfo {
                    pool_name: "pool-C".into(),
                    purpose: "other".into(),
                    nodes: vec![],
                },
            ],
            total_vms: 0,
            running_vms: 0,
        };
        // Simulate load_infra logic (without filesystem)
        if app.tree[infra_idx].children_loaded {
            app.remove_children(infra_idx);
            app.tree[infra_idx].children_loaded = false;
        }
        app.sync_infra_tree();

        let child_count_after = app.tree[(infra_idx + 1)..]
            .iter()
            .take_while(|n| n.depth > app.tree[infra_idx].depth)
            .count();
        assert_eq!(child_count_after, 2); // Updated from 1 to 2
        assert!(app.tree[infra_idx + 1].label.contains("pool-B"));
        assert!(app.tree[infra_idx + 2].label.contains("pool-C"));
    }

    // --- US-081: Left key navigates to parent ---

    #[test]
    fn left_on_namespace_goes_to_parent_cluster() {
        let mut app = test_app();
        // Expand tower with namespace children
        app.tree[1].expanded = true;
        app.tree[1].children_loaded = true;
        app.tree.insert(
            2,
            TreeNode {
                label: "All Namespaces".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "All Namespaces".into(),
                },
                children_loaded: false,
            },
        );
        app.tree.insert(
            3,
            TreeNode {
                label: "kube-system".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "kube-system".into(),
                },
                children_loaded: false,
            },
        );
        // Cursor on kube-system (visible index 3)
        app.tree_cursor = 3;
        app.handle_event(AppEvent::Left);
        // Should navigate to tower (visible index 1)
        assert_eq!(app.tree_cursor, 1);
    }

    #[test]
    fn left_on_collapsed_cluster_goes_to_root() {
        let mut app = test_app();
        // tower is collapsed, cursor on tower (visible index 1)
        app.tree_cursor = 1;
        assert!(!app.tree[1].expanded);
        app.handle_event(AppEvent::Left);
        // Should navigate to Root (visible index 0)
        assert_eq!(app.tree_cursor, 0);
    }

    #[test]
    fn left_on_root_is_noop() {
        let mut app = test_app();
        app.tree_cursor = 0; // on Root
        app.handle_event(AppEvent::Left);
        // Root has depth 0, no parent — stays at 0
        assert_eq!(app.tree_cursor, 0);
    }

    // --- US-082: Expand/collapse no-op on leaf nodes ---

    // --- US-086: Search matches namespace ---

    #[test]
    fn search_matches_namespace() {
        let mut app = test_app();
        app.search_query = Some("kube-system".to_string());
        assert!(app.matches_search_with_ns("coredns", "kube-system"));
        assert!(!app.matches_search_with_ns("coredns", "default"));
    }

    #[test]
    fn search_matches_name_or_namespace() {
        let mut app = test_app();
        app.search_query = Some("core".to_string());
        // Matches name
        assert!(app.matches_search_with_ns("coredns", "default"));
        // Matches namespace
        assert!(app.matches_search_with_ns("nginx", "core-system"));
        // Matches neither
        assert!(!app.matches_search_with_ns("nginx", "default"));
    }

    // --- US-082: Expand/collapse no-op on leaf nodes ---

    #[test]
    fn expand_noop_on_leaf_nodes() {
        let mut app = test_app();
        // Expand tower with namespace children
        app.tree[1].expanded = true;
        app.tree[1].children_loaded = true;
        app.tree.insert(
            2,
            TreeNode {
                label: "default".into(),
                depth: 2,
                expanded: false,
                node_type: NodeType::Namespace {
                    cluster: "tower".into(),
                    namespace: "default".into(),
                },
                children_loaded: false,
            },
        );
        app.tree_cursor = 2; // on namespace
        let tree_len_before = app.tree.len();
        app.handle_event(AppEvent::Right); // expand on leaf
                                           // expanded should NOT be set to true for namespace
        assert!(!app.tree[2].expanded);
        assert_eq!(app.tree.len(), tree_len_before); // no children added
    }

    #[test]
    fn top_tab_scroll_down_increments_offset() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.active_tab = 1; // Top tab
        app.selected_cluster = Some("tower".into());
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec![],
            nodes: vec![
                crate::dash::data::NodeInfo {
                    name: "node-0".into(),
                    status: "Ready".into(),
                    roles: vec![],
                    cpu_capacity: "4".into(),
                    mem_capacity: "8Gi".into(),
                    cpu_allocatable: "4".into(),
                    mem_allocatable: "8Gi".into(),
                },
                crate::dash::data::NodeInfo {
                    name: "node-1".into(),
                    status: "Ready".into(),
                    roles: vec![],
                    cpu_capacity: "4".into(),
                    mem_capacity: "8Gi".into(),
                    cpu_allocatable: "4".into(),
                    mem_allocatable: "8Gi".into(),
                },
            ],
            pods: vec![],
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });
        app.table_scroll_offset = 0;

        app.handle_event(AppEvent::Down);
        assert_eq!(app.table_scroll_offset, 1, "j should scroll Top tab down");

        app.handle_event(AppEvent::Up);
        assert_eq!(app.table_scroll_offset, 0, "k should scroll Top tab up");
    }

    #[test]
    fn top_tab_scroll_capped_at_line_count() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.active_tab = 1;
        app.selected_cluster = Some("tower".into());
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec![],
            nodes: vec![crate::dash::data::NodeInfo {
                name: "node-0".into(),
                status: "Ready".into(),
                roles: vec![],
                cpu_capacity: "4".into(),
                mem_capacity: "8Gi".into(),
                cpu_allocatable: "4".into(),
                mem_allocatable: "8Gi".into(),
            }],
            pods: vec![],
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });
        // top_tab_line_count = 2 + 1 = 3, max scroll = 2
        for _ in 0..20 {
            app.handle_event(AppEvent::Down);
        }
        assert!(
            app.table_scroll_offset < 20,
            "scroll offset should be capped, got {}",
            app.table_scroll_offset
        );
    }

    #[test]
    fn help_scroll_offset_capped() {
        let mut app = test_app();
        app.show_help = true;
        let expected_max = app.help_content_line_count();
        // Press Down 100 times — offset should be capped to content line count
        for _ in 0..100 {
            app.handle_event(AppEvent::Down);
        }
        assert_eq!(
            app.help_scroll_offset, expected_max,
            "help_scroll_offset should be capped at content lines ({}), got {}",
            expected_max, app.help_scroll_offset
        );
    }

    // --- US-070: ensure_table_scroll_visible doesn't reset Top tab scroll ---

    #[test]
    fn ensure_table_scroll_top_tab_preserves_offset() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.active_tab = 1; // Top tab
        app.selected_cluster = Some("tower".into());
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec![],
            nodes: (0..10)
                .map(|i| crate::dash::data::NodeInfo {
                    name: format!("node-{}", i),
                    status: "Ready".into(),
                    roles: vec![],
                    cpu_capacity: "4".into(),
                    mem_capacity: "8Gi".into(),
                    cpu_allocatable: "4".into(),
                    mem_allocatable: "8Gi".into(),
                })
                .collect(),
            pods: vec![],
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });

        // Simulate scrolling down in Top tab
        app.table_scroll_offset = 3;
        // table_cursor stays 0 (unused in Top tab)

        // Before fix: ensure_table_scroll_visible would reset offset to 0
        // because table_cursor(0) < table_scroll_offset(3)
        app.ensure_table_scroll_visible(5);

        assert_eq!(
            app.table_scroll_offset, 3,
            "Top tab scroll offset should be preserved, not reset to table_cursor"
        );
    }

    #[test]
    fn ensure_table_scroll_top_tab_clamps_overscroll() {
        let mut app = test_app();
        app.active_panel = ActivePanel::Center;
        app.active_tab = 1;
        app.selected_cluster = Some("tower".into());
        app.snapshots.push(ClusterSnapshot {
            name: "tower".into(),
            health: HealthStatus::Green,
            namespaces: vec![],
            nodes: vec![crate::dash::data::NodeInfo {
                name: "node-0".into(),
                status: "Ready".into(),
                roles: vec![],
                cpu_capacity: "4".into(),
                mem_capacity: "8Gi".into(),
                cpu_allocatable: "4".into(),
                mem_allocatable: "8Gi".into(),
            }],
            pods: vec![],
            deployments: vec![],
            services: vec![],
            configmaps: vec![],
            resource_usage: Default::default(),
        });
        // top_tab_line_count = 2 + 1 = 3, viewport = 5, max_offset = 0
        app.table_scroll_offset = 10;

        app.ensure_table_scroll_visible(5);

        assert_eq!(
            app.table_scroll_offset, 0,
            "Top tab scroll should clamp to max when content fits viewport"
        );
    }

    // --- US-072: Tab exits search clears partial query ---

    #[test]
    fn tab_exits_search_clears_query() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("partial".into());
        app.active_panel = ActivePanel::Center;

        app.handle_event(AppEvent::NextPanel);

        assert!(!app.search_active);
        assert_eq!(
            app.search_query, None,
            "Tab should clear partial search query"
        );
        assert_eq!(app.active_panel, ActivePanel::Sidebar);
    }

    #[test]
    fn shift_tab_exits_search_clears_query() {
        let mut app = test_app();
        app.search_active = true;
        app.search_query = Some("partial".into());
        app.active_panel = ActivePanel::Center;

        app.handle_event(AppEvent::PrevPanel);

        assert!(!app.search_active);
        assert_eq!(
            app.search_query, None,
            "Shift+Tab should clear partial search query"
        );
    }
}

// ---------------------------------------------------------------------------
// Self-monitoring (Linux only, no external dependencies)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
pub fn read_self_rss_mb() -> Option<f64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| {
                    line.split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse::<f64>().ok())
                        .map(|kb| kb / 1024.0) // kB -> MB
                })
        })
}

#[cfg(not(target_os = "linux"))]
pub fn read_self_rss_mb() -> Option<f64> {
    None
}

// ---------------------------------------------------------------------------
// TUI entry point
// ---------------------------------------------------------------------------

pub async fn run_tui(args: DashArgs, kubeconfig_dir: PathBuf) -> Result<()> {
    // Phase 1: sync scan for cluster names (filesystem only, <100ms)
    let cluster_names = kube_client::scan_kubeconfig_names(&kubeconfig_dir);

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new_with_names(&cluster_names, args.refresh);
    app.load_infra(); // Load infrastructure data once at startup
    let tick_rate = Duration::from_millis(100);
    let refresh_interval = Duration::from_secs(args.refresh);
    let mut last_refresh = Instant::now();

    // Channels for non-blocking communication
    let (discover_tx, mut discover_rx) =
        tokio::sync::mpsc::channel::<kube_client::DiscoverEvent>(32);
    let (fetch_tx, mut fetch_rx) = tokio::sync::mpsc::channel::<FetchResult>(32);

    // Cancellation flag (shared with background tasks)
    let cancelled = Arc::new(AtomicBool::new(false));

    // Phase 2: background cluster discovery (streaming per-cluster results)
    let discover_tx_retry = discover_tx.clone(); // Keep clone for retry (US-201)
    let cancel_discover = cancelled.clone();
    tokio::spawn(async move {
        kube_client::discover_clusters_streaming(kubeconfig_dir, discover_tx, cancel_discover)
            .await;
    });

    let result = loop {
        // Adjust scroll offsets before drawing (US-034, US-035)
        {
            let term_size = terminal.size()?;
            let header_height: u16 = if term_size.height >= 28 { 8 } else { 4 };
            let status_bar_height: u16 = 3;
            let body_height = term_size
                .height
                .saturating_sub(header_height + status_bar_height);
            // Sidebar viewport = body height (right border only, no top/bottom border)
            let sidebar_viewport = body_height as usize;
            app.ensure_sidebar_scroll_visible(sidebar_viewport);
            // Table viewport = body height minus block borders (2) minus header row (1, Resources only) minus optional search bar (1)
            let search_offset: u16 = if app.search_active { 1 } else { 0 };
            let header_row: u16 = if app.active_tab == 0 { 1 } else { 0 }; // US-203: Top tab has no header row
            let table_viewport =
                body_height.saturating_sub(2 + header_row + search_offset) as usize;
            app.ensure_table_scroll_visible(table_viewport);
            // Cache viewport heights for PageUp/PageDown
            app.page_size = table_viewport;
            app.sidebar_page_size = sidebar_viewport;
            // Help popup viewport height (US-204)
            let help_content = app.help_content_line_count();
            let max_popup = term_size.height.saturating_sub(2).max(5);
            let popup_h = (help_content + 2).min(max_popup);
            app.help_viewport_height = popup_h.saturating_sub(2);
        }

        // Draw first — shows skeleton UI instantly on startup (US-004)
        terminal.draw(|f| ui::render(f, &app))?;

        // Handle events (never blocks longer than tick_rate = 100ms)
        let evt = event::poll_event(tick_rate)?;
        app.handle_event(evt);
        app.tick_count += 1;

        if !app.running {
            break Ok(());
        }

        // --- Process discover results (non-blocking) ---
        while let Ok(event) = discover_rx.try_recv() {
            match event {
                kube_client::DiscoverEvent::Connected(client) => {
                    app.tunnel_pids.extend(client.tunnel_pid);
                    let name = client.name.clone();
                    app.cluster_connection_status
                        .insert(name.clone(), ConnectionStatus::Connected);
                    app.clusters.push(client);
                    // Auto-select first connected cluster if none selected
                    if app.selected_cluster.is_none() {
                        app.selected_cluster = Some(name.clone());
                        // Expand the cluster node in sidebar
                        if let Some(idx) = app.tree.iter().position(
                            |n| matches!(&n.node_type, NodeType::Cluster(c) if c == &name),
                        ) {
                            app.tree[idx].expanded = true;
                        }
                    }
                    app.needs_refresh = true;
                    app.fetch_generation += 1;
                    app.is_fetching = false;
                }
                kube_client::DiscoverEvent::Failed { name, error } => {
                    app.cluster_connection_status
                        .insert(name, ConnectionStatus::Failed(error));
                }
                kube_client::DiscoverEvent::Complete => {
                    app.discover_complete = true;
                }
            }
        }

        // --- Process fetch results (non-blocking) ---
        while let Ok(result) = fetch_rx.try_recv() {
            // Discard stale results from a previous generation
            if result.generation != app.fetch_generation {
                app.is_fetching = false;
                app.fetch_started_at = None;
                continue;
            }
            match result.active_resource {
                None => {
                    // Full fetch — replace everything
                    app.snapshots = result.snapshots;
                }
                Some(active) => {
                    // Selective fetch — merge only the fetched resource into existing snapshots
                    for new_snap in result.snapshots {
                        if let Some(existing) =
                            app.snapshots.iter_mut().find(|s| s.name == new_snap.name)
                        {
                            // Always update namespaces and nodes (fetched every cycle)
                            existing.namespaces = new_snap.namespaces;
                            existing.nodes = new_snap.nodes;
                            // Only update the actively fetched resource
                            match active {
                                ActiveResource::Pods => existing.pods = new_snap.pods,
                                ActiveResource::Deployments => {
                                    existing.deployments = new_snap.deployments
                                }
                                ActiveResource::Services => existing.services = new_snap.services,
                                ActiveResource::ConfigMaps => {
                                    existing.configmaps = new_snap.configmaps
                                }
                                ActiveResource::Nodes => {} // nodes already updated above
                            }
                            // Recompute health from merged nodes + pods
                            existing.health = data::compute_health(&existing.nodes, &existing.pods);
                            existing.resource_usage =
                                data::compute_resource_usage(&existing.nodes, &existing.pods, None);
                        } else {
                            // New cluster not yet in snapshots — add it
                            app.snapshots.push(new_snap);
                        }
                    }
                }
            }
            app.api_latency_ms = result.latency_ms;
            app.last_fetched_resource = result.active_resource;
            app.sync_tree_from_snapshots();
            app.self_rss_mb = read_self_rss_mb();
            app.is_fetching = false;
            app.fetch_started_at = None;
            app.fetch_timed_out = false;
            // Clamp table cursor after data change (US-030, US-062: respects search filter)
            let row_count = app.current_row_count();
            app.clamp_table_cursor(row_count);
            last_refresh = Instant::now();
        }

        // --- is_fetching timeout defense (30s) ---
        if let Some(started) = app.fetch_started_at {
            if started.elapsed() > Duration::from_secs(30) {
                app.is_fetching = false;
                app.fetch_started_at = None;
                app.fetch_timed_out = true;
            }
        }

        // --- Retry failed cluster discovery on manual refresh (US-201) ---
        if app.needs_refresh {
            let failed_names: Vec<String> = app
                .cluster_connection_status
                .iter()
                .filter(|(_, s)| matches!(s, ConnectionStatus::Failed(_)))
                .map(|(n, _)| n.clone())
                .collect();
            if !failed_names.is_empty() {
                // Reset failed clusters to Discovering and re-spawn discovery
                for name in &failed_names {
                    app.cluster_connection_status
                        .insert(name.clone(), ConnectionStatus::Discovering);
                }
                let dir = args
                    .kubeconfig_dir
                    .clone()
                    .unwrap_or_else(|| std::path::PathBuf::from("_generated/clusters"));
                let retry_tx = discover_tx_retry.clone();
                let retry_cancel = cancelled.clone();
                let retry_names = failed_names;
                tokio::spawn(async move {
                    kube_client::discover_clusters_streaming_filtered(
                        dir,
                        retry_tx,
                        retry_cancel,
                        &retry_names,
                    )
                    .await;
                });
            }
        }

        // --- Trigger background fetch if needed ---
        let was_manual_refresh = app.needs_refresh;
        if !app.is_fetching
            && !app.clusters.is_empty()
            && (last_refresh.elapsed() >= refresh_interval || app.needs_refresh)
        {
            // Reload infra data on manual refresh (r key)
            if was_manual_refresh {
                app.load_infra();
            }
            app.needs_refresh = false;
            app.is_fetching = true;
            app.fetch_started_at = Some(Instant::now());

            let tx = fetch_tx.clone();
            // Clone only (name, client) pairs — avoid cloning kubeconfig_path, endpoint, etc.
            let cluster_refs: Vec<(String, kube::Client)> = app
                .clusters
                .iter()
                .map(|c| (c.name.clone(), c.client.clone()))
                .collect();
            let selected_cluster = app.selected_cluster.clone();
            let ns = app.selected_namespace.clone();
            let cancel = cancelled.clone();
            let active_res = Some(app.resource_view.to_active_resource());
            let generation = app.fetch_generation;

            tokio::spawn(async move {
                let start = Instant::now();
                let mut handles = Vec::new();
                for (name, client) in &cluster_refs {
                    let client = client.clone();
                    let name = name.clone();
                    // Only apply namespace filter to the selected cluster;
                    // other clusters fetch all namespaces for accurate status bar counts
                    let cluster_ns = if selected_cluster.as_ref() == Some(&name) {
                        ns.clone()
                    } else {
                        None
                    };
                    handles.push(tokio::spawn(async move {
                        match data::fetch_cluster_snapshot(
                            &client,
                            &name,
                            cluster_ns.as_deref(),
                            active_res,
                        )
                        .await
                        {
                            Ok(snapshot) => snapshot,
                            Err(_) => ClusterSnapshot {
                                name,
                                health: HealthStatus::Unknown,
                                namespaces: vec![],
                                nodes: vec![],
                                pods: vec![],
                                deployments: vec![],
                                services: vec![],
                                configmaps: vec![],
                                resource_usage: Default::default(),
                            },
                        }
                    }));
                }

                let mut snapshots = Vec::new();
                for handle in handles {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Ok(snapshot) = handle.await {
                        snapshots.push(snapshot);
                    }
                }
                let latency_ms = start.elapsed().as_millis() as u64;
                let _ = tx
                    .send(FetchResult {
                        snapshots,
                        latency_ms,
                        active_resource: active_res,
                        generation,
                    })
                    .await;
            });
        }
    };

    // Signal cancellation to background tasks
    cancelled.store(true, Ordering::Relaxed);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Cleanup all SSH tunnels (panic-safe: tunnel_pids accumulated during run)
    for &pid in &app.tunnel_pids {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }

    result
}
