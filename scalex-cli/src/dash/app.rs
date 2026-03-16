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

    // Center table scroll
    pub table_cursor: usize,

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
            table_cursor: 0,
            selected_cluster: None,
            selected_namespace: None,
            clusters,
            snapshots: Vec::new(),
            infra: InfraSnapshot::default(),
            api_latency_ms: 0,
            show_help: false,
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
            table_cursor: 0,
            selected_cluster: None,
            selected_namespace: None,
            clusters: Vec::new(),
            snapshots: Vec::new(),
            infra: InfraSnapshot::default(),
            api_latency_ms: 0,
            show_help: false,
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
        }
    }

    pub fn handle_event(&mut self, evt: AppEvent) {
        // Search-mode: intercept all events as text input (mirrors show_help pattern)
        if self.search_active {
            match evt {
                AppEvent::Enter => {
                    // Submit search — keep query as filter, exit search mode
                    self.search_active = false;
                }
                AppEvent::Quit | AppEvent::Help | AppEvent::Escape => {
                    // Cancel search — clear query and exit
                    self.search_active = false;
                    self.search_query = None;
                    self.table_cursor = 0;
                }
                AppEvent::ResourceType(c) => {
                    // In search mode, treat as literal character
                    self.search_query.get_or_insert_with(String::new).push(c);
                    self.table_cursor = 0;
                }
                AppEvent::Up => {
                    self.search_query.get_or_insert_with(String::new).push('k');
                }
                AppEvent::Down => {
                    self.search_query.get_or_insert_with(String::new).push('j');
                }
                AppEvent::Left => {
                    // Backspace behavior in search
                    if let Some(q) = &mut self.search_query {
                        q.pop();
                        if q.is_empty() {
                            self.search_query = None;
                        }
                    }
                    self.table_cursor = 0;
                }
                AppEvent::Refresh => {
                    self.search_query.get_or_insert_with(String::new).push('r');
                }
                AppEvent::Search => {
                    self.search_query.get_or_insert_with(String::new).push('/');
                }
                _ => {}
            }
            return;
        }

        // show_help and search_active are mutually exclusive by construction:
        // search intercepts Help/Quit (above), and help blocks Search (below).
        if self.show_help {
            if matches!(evt, AppEvent::Help | AppEvent::Quit | AppEvent::Enter | AppEvent::Escape) {
                self.show_help = false;
            }
            return;
        }

        match evt {
            AppEvent::Quit => self.running = false,
            AppEvent::Up => self.move_up(),
            AppEvent::Down => self.move_down(),
            AppEvent::Enter => self.handle_enter(),
            AppEvent::Left => self.collapse_node(),
            AppEvent::Right => self.expand_node(),
            AppEvent::Tab(n) => {
                if n > 0 && n <= self.tabs.len() {
                    self.active_tab = n - 1;
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
                if self.active_panel == ActivePanel::Center {
                    if let Some(rv) = ResourceView::from_char(c) {
                        if self.resource_view != rv {
                            self.resource_view = rv;
                            self.table_cursor = 0;
                            self.needs_refresh = true;
                            self.fetch_generation += 1;
                            self.is_fetching = false;
                        }
                    }
                }
            }
            AppEvent::Help => self.show_help = true,
            AppEvent::Search => {
                // Activate search mode
                self.search_active = true;
                self.search_query = Some(String::new());
                self.table_cursor = 0;
            }
            AppEvent::Refresh | AppEvent::Tick | AppEvent::None | AppEvent::Escape => {}
        }
    }

    /// Check if a name matches the current search query (case-insensitive)
    pub fn matches_search(&self, name: &str) -> bool {
        match &self.search_query {
            Some(q) if !q.is_empty() => name.to_lowercase().contains(&q.to_lowercase()),
            _ => true,
        }
    }

    fn move_up(&mut self) {
        match self.active_panel {
            ActivePanel::Sidebar => {
                if self.tree_cursor > 0 {
                    self.tree_cursor -= 1;
                }
            }
            ActivePanel::Center => {
                if self.table_cursor > 0 {
                    self.table_cursor -= 1;
                }
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
            }
            ActivePanel::Center => {
                self.table_cursor += 1; // UI will clamp
            }
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
                    // Collapse: remove children
                    self.tree[idx].expanded = false;
                    self.remove_children(idx);
                } else {
                    self.tree[idx].expanded = true;
                    self.selected_cluster = Some(name.clone());
                    self.selected_namespace = None;
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
                self.needs_refresh = true;
                self.fetch_generation += 1;
                self.is_fetching = false;
            }
            NodeType::InfraHeader => {
                self.tree[idx].expanded = !self.tree[idx].expanded;
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
        if self.tree[idx].expanded {
            self.tree[idx].expanded = false;
            self.remove_children(idx);
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
        if !self.tree[idx].expanded {
            self.tree[idx].expanded = true;
            // Note: no selection change — expand only. Use Enter to select.
        }
    }

    fn remove_children(&mut self, parent_idx: usize) {
        let parent_depth = self.tree[parent_idx].depth;
        while parent_idx + 1 < self.tree.len() && self.tree[parent_idx + 1].depth > parent_depth {
            self.tree.remove(parent_idx + 1);
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
        self.visible_tree_indices().len()
    }

    /// Load infrastructure data from SDI directory
    pub fn load_infra(&mut self) {
        let sdi_dir = std::path::Path::new("_generated/sdi");
        self.infra = infra::load_sdi_state(sdi_dir);
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

                let insert_at = idx + 1;
                for (j, child) in children.into_iter().enumerate() {
                    self.tree.insert(insert_at + j, child);
                }
                self.tree[idx].children_loaded = true;
            }
        }
    }

    /// Populate namespace children for expanded clusters from snapshot data
    pub fn sync_tree_from_snapshots(&mut self) {
        for snapshot in &self.snapshots {
            // Find the cluster node
            let cluster_idx = self.tree.iter().position(
                |n| matches!(&n.node_type, NodeType::Cluster(name) if name == &snapshot.name),
            );

            if let Some(idx) = cluster_idx {
                if self.tree[idx].expanded && !self.tree[idx].children_loaded {
                    let depth = self.tree[idx].depth + 1;
                    let cluster_name = snapshot.name.clone();

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

                    for ns in &snapshot.namespaces {
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

                    // Insert children after cluster node
                    let insert_at = idx + 1;
                    for (j, child) in children.into_iter().enumerate() {
                        self.tree.insert(insert_at + j, child);
                    }

                    self.tree[idx].children_loaded = true;
                }
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

    /// Get current cluster's snapshot
    pub fn current_snapshot(&self) -> Option<&ClusterSnapshot> {
        self.selected_cluster
            .as_ref()
            .and_then(|name| self.snapshots.iter().find(|s| &s.name == name))
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
            table_cursor: 0,
            selected_cluster: None,
            selected_namespace: None,
            clusters: vec![],
            snapshots: vec![],
            infra: InfraSnapshot::default(),
            api_latency_ms: 0,
            show_help: false,
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
    let cancel_discover = cancelled.clone();
    tokio::spawn(async move {
        kube_client::discover_clusters_streaming(kubeconfig_dir, discover_tx, cancel_discover)
            .await;
    });

    let result = loop {
        // Draw first — shows skeleton UI instantly on startup (US-004)
        terminal.draw(|f| ui::render(f, &app))?;

        // Handle events (never blocks longer than tick_rate = 100ms)
        let evt = event::poll_event(tick_rate)?;
        if evt == AppEvent::Refresh {
            app.needs_refresh = true;
        }
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
                    app.cluster_connection_status
                        .insert(client.name.clone(), ConnectionStatus::Connected);
                    app.clusters.push(client);
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
                            // Always update namespaces, nodes, health, resource_usage
                            existing.namespaces = new_snap.namespaces;
                            existing.nodes = new_snap.nodes;
                            existing.health = new_snap.health;
                            existing.resource_usage = new_snap.resource_usage;
                            // Only update the actively fetched resource
                            match active {
                                ActiveResource::Pods => {
                                    existing.pods = new_snap.pods;
                                    // Recompute health with fresh pods
                                    existing.health =
                                        data::compute_health(&existing.nodes, &existing.pods);
                                    existing.resource_usage = data::compute_resource_usage(
                                        &existing.nodes,
                                        &existing.pods,
                                        None,
                                    );
                                }
                                ActiveResource::Deployments => {
                                    existing.deployments = new_snap.deployments
                                }
                                ActiveResource::Services => existing.services = new_snap.services,
                                ActiveResource::ConfigMaps => {
                                    existing.configmaps = new_snap.configmaps
                                }
                                ActiveResource::Nodes => {} // nodes already updated above
                            }
                            // For non-pod fetches, recompute health using existing pods
                            if active != ActiveResource::Pods {
                                existing.health =
                                    data::compute_health(&existing.nodes, &existing.pods);
                                existing.resource_usage = data::compute_resource_usage(
                                    &existing.nodes,
                                    &existing.pods,
                                    None,
                                );
                            }
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
            app.load_infra();
            app.self_rss_mb = read_self_rss_mb();
            app.is_fetching = false;
            app.fetch_started_at = None;
            last_refresh = Instant::now();
        }

        // --- is_fetching timeout defense (30s) ---
        if let Some(started) = app.fetch_started_at {
            if started.elapsed() > Duration::from_secs(30) {
                app.is_fetching = false;
                app.fetch_started_at = None;
            }
        }

        // --- Trigger background fetch if needed ---
        if !app.is_fetching
            && !app.clusters.is_empty()
            && (last_refresh.elapsed() >= refresh_interval || app.needs_refresh)
        {
            app.needs_refresh = false;
            app.is_fetching = true;
            app.fetch_started_at = Some(Instant::now());

            let tx = fetch_tx.clone();
            let clusters = app.clusters.clone();
            let ns = app.selected_namespace.clone();
            let cancel = cancelled.clone();
            let active_res = Some(app.resource_view.to_active_resource());
            let generation = app.fetch_generation;

            tokio::spawn(async move {
                let start = Instant::now();
                let mut handles = Vec::new();
                for cluster in &clusters {
                    let client = cluster.client.clone();
                    let name = cluster.name.clone();
                    let ns = ns.clone();
                    handles.push(tokio::spawn(async move {
                        match data::fetch_cluster_snapshot(
                            &client,
                            &name,
                            ns.as_deref(),
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
