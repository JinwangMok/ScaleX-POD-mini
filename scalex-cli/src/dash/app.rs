use crate::commands::dash::DashArgs;
use crate::dash::data::{self, ClusterSnapshot, HealthStatus};
use crate::dash::event::{self, AppEvent};
use crate::dash::infra::{self, InfraSnapshot};
use crate::dash::kube_client::ClusterClient;
use crate::dash::ui;
use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
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
}

impl App {
    pub fn new(clusters: Vec<ClusterClient>, refresh_secs: u64) -> Self {
        let mut tree = vec![TreeNode {
            label: "ScaleX-POD".to_string(),
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
                AppEvent::Quit | AppEvent::Help => {
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

        if self.show_help {
            if matches!(evt, AppEvent::Help | AppEvent::Quit | AppEvent::Enter) {
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
                        self.resource_view = rv;
                        self.table_cursor = 0;
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
            AppEvent::Refresh | AppEvent::Tick | AppEvent::None => {}
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
                    // Children will be populated on next data refresh
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
            if let NodeType::Cluster(name) = &self.tree[idx].node_type {
                self.selected_cluster = Some(name.clone());
            }
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

    /// Get current cluster's snapshot
    pub fn current_snapshot(&self) -> Option<&ClusterSnapshot> {
        self.selected_cluster
            .as_ref()
            .and_then(|name| self.snapshots.iter().find(|s| &s.name == name))
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

pub async fn run_tui(args: DashArgs, clusters: Vec<ClusterClient>) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(clusters.clone(), args.refresh);
    let tick_rate = Duration::from_millis(100);
    let refresh_interval = Duration::from_secs(args.refresh);
    let mut last_refresh = Instant::now() - refresh_interval; // trigger immediate refresh

    let result = loop {
        // Refresh data if needed
        if last_refresh.elapsed() >= refresh_interval {
            let start = Instant::now();
            let mut snapshots = Vec::new();
            for cluster in &clusters {
                match data::fetch_cluster_snapshot(
                    &cluster.client,
                    &cluster.name,
                    app.selected_namespace.as_deref(),
                )
                .await
                {
                    Ok(snapshot) => snapshots.push(snapshot),
                    Err(e) => {
                        snapshots.push(ClusterSnapshot {
                            name: cluster.name.clone(),
                            health: HealthStatus::Unknown,
                            namespaces: vec![],
                            nodes: vec![],
                            pods: vec![],
                            deployments: vec![],
                            services: vec![],
                            configmaps: vec![],
                            resource_usage: Default::default(),
                        });
                        let _ = e; // logged in status bar as Unknown health
                    }
                }
            }
            app.api_latency_ms = start.elapsed().as_millis() as u64;
            app.snapshots = snapshots;
            app.sync_tree_from_snapshots();
            app.load_infra();
            app.self_rss_mb = read_self_rss_mb();
            last_refresh = Instant::now();
        }

        // Draw
        terminal.draw(|f| ui::render(f, &app))?;

        // Handle events
        let evt = event::poll_event(tick_rate)?;
        if evt == AppEvent::Refresh {
            last_refresh = Instant::now() - refresh_interval; // force refresh
        }
        app.handle_event(evt);

        if !app.running {
            break Ok(());
        }
    };

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
