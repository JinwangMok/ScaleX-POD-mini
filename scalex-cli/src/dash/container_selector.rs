//! Container selector sub-modal — detects multi-container pods, lists
//! init and regular containers, and allows selection before opening logs
//! or an exec shell.
//!
//! In k9s, pressing `l` on a multi-container pod shows a container picker.
//! If only one container exists, the log viewer opens directly (no picker).
//!
//! # Key bindings (when container selector is visible)
//!
//! | Key      | Action                       |
//! |----------|------------------------------|
//! | `j`/`↓`  | Move cursor down             |
//! | `k`/`↑`  | Move cursor up               |
//! | `Enter`  | Select container & confirm   |
//! | `ESC`/`q`| Close selector               |

use crate::dash::theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// ContainerAction — what to do with the selected container
// ---------------------------------------------------------------------------

/// Action to perform after a container is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerAction {
    /// View container logs.
    Logs,
    /// Open an interactive shell / exec session.
    ShellExec,
}

impl ContainerAction {
    /// Human-readable label for the action.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Logs => "Logs",
            Self::ShellExec => "Shell",
        }
    }
}

// ---------------------------------------------------------------------------
// ContainerInfo — describes a single container in a pod
// ---------------------------------------------------------------------------

/// Metadata for a container shown in the selector list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInfo {
    /// Container name.
    pub name: String,
    /// Whether this is an init container.
    pub is_init: bool,
    /// Container status string (e.g., "Running", "Terminated", "Waiting").
    pub status: String,
    /// Container image (truncated for display).
    pub image: String,
    /// Number of restarts.
    pub restarts: u32,
}

// ---------------------------------------------------------------------------
// ContainerSelection — the result of a container selection
// ---------------------------------------------------------------------------

/// Result from the container selector, consumed by the log viewer or exec handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSelection {
    /// Pod name.
    pub pod_name: String,
    /// Namespace.
    pub namespace: String,
    /// Selected container name.
    pub container_name: String,
    /// Action to perform on the container.
    pub action: ContainerAction,
}

// ---------------------------------------------------------------------------
// ContainerSelector state
// ---------------------------------------------------------------------------

/// State for the container selector sub-modal.
///
/// When `visible` is true, this modal intercepts keyboard events and renders
/// a compact list of containers over the center panel. Selecting a container
/// closes the modal and returns the selection for the caller to consume.
#[derive(Debug, Clone)]
pub struct ContainerSelector {
    /// Whether the modal is currently shown.
    pub visible: bool,
    /// Pod name this selector is for.
    pub pod_name: String,
    /// Pod namespace.
    pub namespace: String,
    /// List of containers (init containers first, then regular).
    pub containers: Vec<ContainerInfo>,
    /// Current cursor position in the list.
    pub cursor: usize,
    /// If set, the user selected a container index. Consumed by the caller.
    pub selected: Option<usize>,
    /// Action to perform on the selected container.
    pub action: ContainerAction,
}

impl ContainerSelector {
    /// Create a new hidden container selector.
    pub fn new() -> Self {
        Self {
            visible: false,
            pod_name: String::new(),
            namespace: String::new(),
            containers: Vec::new(),
            cursor: 0,
            selected: None,
            action: ContainerAction::Logs,
        }
    }

    /// Open the container selector for a pod with the default action (Logs).
    ///
    /// Init containers are displayed first, followed by regular containers.
    /// If `containers` has only one entry, the caller should skip the selector
    /// and open the log viewer directly.
    pub fn open(&mut self, pod_name: String, namespace: String, containers: Vec<ContainerInfo>) {
        self.open_with_action(pod_name, namespace, containers, ContainerAction::Logs);
    }

    /// Open the container selector with an explicit action.
    pub fn open_with_action(
        &mut self,
        pod_name: String,
        namespace: String,
        containers: Vec<ContainerInfo>,
        action: ContainerAction,
    ) {
        self.pod_name = pod_name;
        self.namespace = namespace;
        self.containers = containers;
        self.cursor = 0;
        self.selected = None;
        self.action = action;
        self.visible = true;
    }

    /// Close the selector without making a selection.
    pub fn close(&mut self) {
        self.visible = false;
        self.containers.clear();
        self.cursor = 0;
        self.selected = None;
    }

    /// Move cursor up by one.
    pub fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor down by one.
    pub fn move_down(&mut self) {
        if !self.containers.is_empty() && self.cursor < self.containers.len() - 1 {
            self.cursor += 1;
        }
    }

    /// Confirm the current selection. Returns `true` if a container was
    /// selected, `false` if the container list is empty.
    pub fn confirm(&mut self) -> bool {
        if self.containers.is_empty() {
            return false;
        }
        self.selected = Some(self.cursor);
        self.visible = false;
        true
    }

    /// Take the pending selection (if any), consuming it.
    pub fn take_selection(&mut self) -> Option<ContainerSelection> {
        let idx = self.selected.take()?;
        let container = self.containers.get(idx)?;
        Some(ContainerSelection {
            pod_name: self.pod_name.clone(),
            namespace: self.namespace.clone(),
            container_name: container.name.clone(),
            action: self.action,
        })
    }

    /// Returns the number of containers.
    pub fn container_count(&self) -> usize {
        self.containers.len()
    }

    // -- Rendering ---------------------------------------------------------

    /// Render the container selector modal overlay.
    /// Uses a compact centered popup with dimmed background.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        if !self.visible || self.containers.is_empty() {
            return;
        }

        // Compute popup dimensions
        let popup_height = (self.containers.len() as u16 + 5)
            .min(20)
            .min(area.height.saturating_sub(4));
        let popup_width = 70u16.min(area.width.saturating_sub(4)).max(40);

        if popup_height < 5 || popup_width < 30 {
            return; // Terminal too small
        }

        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        // Dim background
        f.render_widget(DimOverlay, area);

        // Clear popup area
        f.render_widget(Clear, popup_area);

        // Draw border
        let title = format!(
            " {} Container \u{2014} {} ",
            self.action.label(),
            self.pod_name
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::BRIGHT_YELLOW))
            .style(Style::default().bg(theme::BG));
        f.render_widget(block, popup_area);

        // Inner content area
        let inner = Rect::new(
            popup_area.x + 1,
            popup_area.y + 1,
            popup_area.width.saturating_sub(2),
            popup_area.height.saturating_sub(2),
        );

        let mut lines: Vec<Line<'_>> = Vec::new();

        // Header row
        lines.push(Line::from(vec![Span::styled(
            format!(
                " {:20} {:10} {:10} {}",
                "CONTAINER", "TYPE", "STATUS", "RESTARTS"
            ),
            Style::default()
                .fg(theme::BRIGHT_YELLOW)
                .add_modifier(Modifier::BOLD),
        )]));

        // Separator
        let sep_len = inner.width.saturating_sub(2) as usize;
        lines.push(Line::from(Span::styled(
            format!(" {}", "\u{2500}".repeat(sep_len)),
            Style::default().fg(theme::BG3),
        )));

        // Container rows
        for (i, c) in self.containers.iter().enumerate() {
            let is_selected = i == self.cursor;
            let type_label = if c.is_init { "init" } else { "container" };

            let status_style = match c.status.as_str() {
                "Running" => Style::default().fg(theme::BRIGHT_GREEN),
                "Terminated" | "Completed" => Style::default().fg(theme::BRIGHT_RED),
                "Waiting" | "CrashLoopBackOff" => Style::default().fg(theme::BRIGHT_YELLOW),
                _ => Style::default().fg(theme::FG4),
            };

            let (indicator, base_style) = if is_selected {
                (
                    "\u{25b8}",
                    Style::default()
                        .fg(theme::BRIGHT_AQUA)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (" ", Style::default().fg(theme::FG))
            };

            let status_display = if c.status.is_empty() { "-" } else { &c.status };

            lines.push(Line::from(vec![
                Span::styled(format!("{} ", indicator), base_style),
                Span::styled(format!("{:20}", c.name), base_style),
                Span::styled(
                    format!("{:10}", type_label),
                    if is_selected {
                        base_style
                    } else {
                        Style::default().fg(theme::FG4)
                    },
                ),
                Span::styled(
                    format!("{:10}", status_display),
                    if is_selected {
                        base_style
                    } else {
                        status_style
                    },
                ),
                Span::styled(
                    c.restarts.to_string(),
                    if is_selected {
                        base_style
                    } else {
                        Style::default().fg(theme::FG4)
                    },
                ),
            ]));
        }

        // Footer separator + hints
        lines.push(Line::from(Span::styled(
            format!(" {}", "\u{2500}".repeat(sep_len)),
            Style::default().fg(theme::BG3),
        )));
        lines.push(Line::from(vec![Span::styled(
            " \u{2191}\u{2193}/jk: navigate  Enter: select  ESC: cancel",
            Style::default().fg(theme::FG4),
        )]));

        let paragraph = Paragraph::new(lines).style(Style::default().bg(theme::BG));
        f.render_widget(paragraph, inner);
    }
}

impl Default for ContainerSelector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// DimOverlay — simple background dimmer
// ---------------------------------------------------------------------------

struct DimOverlay;

impl Widget for DimOverlay {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let dim_style = Style::default()
            .bg(ratatui::style::Color::Rgb(20, 20, 20))
            .fg(ratatui::style::Color::Rgb(60, 60, 60));
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(dim_style);
                    cell.set_char(' ');
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: extract containers from a kube-rs Pod object
// ---------------------------------------------------------------------------

/// Extract container info from a `k8s_openapi::api::core::v1::Pod`.
///
/// Returns init containers first, then regular containers, each with
/// status information extracted from the pod status.
pub fn extract_containers(pod: &k8s_openapi::api::core::v1::Pod) -> Vec<ContainerInfo> {
    let mut result = Vec::new();

    let spec = match pod.spec.as_ref() {
        Some(s) => s,
        None => return result,
    };

    let status = pod.status.as_ref();

    // Init containers
    if let Some(init_containers) = &spec.init_containers {
        let init_statuses = status
            .and_then(|s| s.init_container_statuses.as_ref())
            .cloned()
            .unwrap_or_default();

        for ic in init_containers {
            let name = ic.name.clone();
            let image = ic.image.clone().unwrap_or_default();
            let cs = init_statuses.iter().find(|s| s.name == name);
            let (status_str, restarts) = container_status_info(cs);

            result.push(ContainerInfo {
                name,
                is_init: true,
                status: status_str,
                image,
                restarts,
            });
        }
    }

    // Regular containers
    let container_statuses = status
        .and_then(|s| s.container_statuses.as_ref())
        .cloned()
        .unwrap_or_default();

    for c in &spec.containers {
        let name = c.name.clone();
        let image = c.image.clone().unwrap_or_default();
        let cs = container_statuses.iter().find(|s| s.name == name);
        let (status_str, restarts) = container_status_info(cs);

        result.push(ContainerInfo {
            name,
            is_init: false,
            status: status_str,
            image,
            restarts,
        });
    }

    result
}

/// Extract container info from a DynamicObject's JSON data.
///
/// Used when viewing pods through the dynamic resource / command-mode path.
pub fn extract_containers_from_value(obj: &serde_json::Value) -> Vec<ContainerInfo> {
    let mut result = Vec::new();

    let spec = match obj.get("spec") {
        Some(s) => s,
        None => return result,
    };
    let status = obj.get("status");

    // Init containers
    if let Some(init_containers) = spec.get("initContainers").and_then(|v| v.as_array()) {
        let init_statuses = status
            .and_then(|s| s.get("initContainerStatuses"))
            .and_then(|v| v.as_array());

        for ic in init_containers {
            let name = ic
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let image = ic
                .get("image")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cs = init_statuses.and_then(|statuses| {
                statuses
                    .iter()
                    .find(|s| s.get("name").and_then(|n| n.as_str()) == Some(&name))
            });
            let (status_str, restarts) = container_status_from_value(cs);

            result.push(ContainerInfo {
                name,
                is_init: true,
                status: status_str,
                image,
                restarts,
            });
        }
    }

    // Regular containers
    if let Some(containers) = spec.get("containers").and_then(|v| v.as_array()) {
        let container_statuses = status
            .and_then(|s| s.get("containerStatuses"))
            .and_then(|v| v.as_array());

        for c in containers {
            let name = c
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let image = c
                .get("image")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cs = container_statuses.and_then(|statuses| {
                statuses
                    .iter()
                    .find(|s| s.get("name").and_then(|n| n.as_str()) == Some(&name))
            });
            let (status_str, restarts) = container_status_from_value(cs);

            result.push(ContainerInfo {
                name,
                is_init: false,
                status: status_str,
                image,
                restarts,
            });
        }
    }

    result
}

/// Extract status string and restart count from a typed ContainerStatus.
fn container_status_info(
    cs: Option<&k8s_openapi::api::core::v1::ContainerStatus>,
) -> (String, u32) {
    match cs {
        Some(s) => {
            let restarts = s.restart_count as u32;
            let state = s.state.as_ref();
            let status_str = if let Some(st) = state {
                if st.running.is_some() {
                    "Running".to_string()
                } else if let Some(term) = &st.terminated {
                    term.reason
                        .clone()
                        .unwrap_or_else(|| "Terminated".to_string())
                } else if let Some(wait) = &st.waiting {
                    wait.reason.clone().unwrap_or_else(|| "Waiting".to_string())
                } else {
                    "Unknown".to_string()
                }
            } else {
                "Unknown".to_string()
            };
            (status_str, restarts)
        }
        None => ("Pending".to_string(), 0),
    }
}

/// Extract status string and restart count from a JSON container status value.
fn container_status_from_value(cs: Option<&serde_json::Value>) -> (String, u32) {
    match cs {
        Some(s) => {
            let restarts = s.get("restartCount").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let state = s.get("state");
            let status_str = if let Some(st) = state {
                if st.get("running").is_some() {
                    "Running".to_string()
                } else if let Some(term) = st.get("terminated") {
                    term.get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Terminated")
                        .to_string()
                } else if let Some(wait) = st.get("waiting") {
                    wait.get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Waiting")
                        .to_string()
                } else {
                    "Unknown".to_string()
                }
            } else {
                "Unknown".to_string()
            };
            (status_str, restarts)
        }
        None => ("Pending".to_string(), 0),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_containers() -> Vec<ContainerInfo> {
        vec![
            ContainerInfo {
                name: "nginx".to_string(),
                is_init: false,
                status: "Running".to_string(),
                image: "nginx:latest".to_string(),
                restarts: 0,
            },
            ContainerInfo {
                name: "sidecar".to_string(),
                is_init: false,
                status: "Running".to_string(),
                image: "envoy:1.0".to_string(),
                restarts: 2,
            },
            ContainerInfo {
                name: "init-db".to_string(),
                is_init: true,
                status: "Completed".to_string(),
                image: "migrate:v1".to_string(),
                restarts: 0,
            },
        ]
    }

    // -- ContainerAction ----------------------------------------------------

    #[test]
    fn action_labels() {
        assert_eq!(ContainerAction::Logs.label(), "Logs");
        assert_eq!(ContainerAction::ShellExec.label(), "Shell");
    }

    #[test]
    fn action_equality() {
        assert_eq!(ContainerAction::Logs, ContainerAction::Logs);
        assert_ne!(ContainerAction::Logs, ContainerAction::ShellExec);
    }

    // -- ContainerSelector construction / state -----------------------------

    #[test]
    fn new_selector_is_hidden() {
        let sel = ContainerSelector::new();
        assert!(!sel.visible);
        assert!(sel.containers.is_empty());
        assert_eq!(sel.cursor, 0);
        assert!(sel.selected.is_none());
        assert_eq!(sel.action, ContainerAction::Logs);
    }

    #[test]
    fn default_is_same_as_new() {
        let sel = ContainerSelector::default();
        assert!(!sel.visible);
        assert!(sel.containers.is_empty());
        assert_eq!(sel.action, ContainerAction::Logs);
    }

    #[test]
    fn open_sets_state_with_default_action() {
        let mut sel = ContainerSelector::new();
        sel.open(
            "my-pod".to_string(),
            "default".to_string(),
            sample_containers(),
        );
        assert!(sel.visible);
        assert_eq!(sel.pod_name, "my-pod");
        assert_eq!(sel.namespace, "default");
        assert_eq!(sel.containers.len(), 3);
        assert_eq!(sel.cursor, 0);
        assert!(sel.selected.is_none());
        assert_eq!(sel.action, ContainerAction::Logs);
    }

    #[test]
    fn open_with_action_sets_action() {
        let mut sel = ContainerSelector::new();
        sel.open_with_action(
            "pod-x".to_string(),
            "kube-system".to_string(),
            sample_containers(),
            ContainerAction::ShellExec,
        );
        assert!(sel.visible);
        assert_eq!(sel.action, ContainerAction::ShellExec);
        assert_eq!(sel.pod_name, "pod-x");
    }

    #[test]
    fn close_clears_state() {
        let mut sel = ContainerSelector::new();
        sel.open("pod".to_string(), "ns".to_string(), sample_containers());
        sel.close();
        assert!(!sel.visible);
        assert!(sel.containers.is_empty());
        assert_eq!(sel.cursor, 0);
        assert!(sel.selected.is_none());
    }

    // -- Navigation ---------------------------------------------------------

    #[test]
    fn move_down_increments_cursor() {
        let mut sel = ContainerSelector::new();
        sel.open("pod".to_string(), "ns".to_string(), sample_containers());
        assert_eq!(sel.cursor, 0);
        sel.move_down();
        assert_eq!(sel.cursor, 1);
        sel.move_down();
        assert_eq!(sel.cursor, 2);
    }

    #[test]
    fn move_down_clamps_at_bottom() {
        let mut sel = ContainerSelector::new();
        sel.open(
            "pod".to_string(),
            "ns".to_string(),
            vec![ContainerInfo {
                name: "a".into(),
                is_init: false,
                status: "Running".into(),
                image: "img".into(),
                restarts: 0,
            }],
        );
        sel.move_down();
        assert_eq!(sel.cursor, 0);
    }

    #[test]
    fn move_up_decrements_cursor() {
        let mut sel = ContainerSelector::new();
        sel.open("pod".to_string(), "ns".to_string(), sample_containers());
        sel.cursor = 2;
        sel.move_up();
        assert_eq!(sel.cursor, 1);
        sel.move_up();
        assert_eq!(sel.cursor, 0);
    }

    #[test]
    fn move_up_clamps_at_top() {
        let mut sel = ContainerSelector::new();
        sel.open("pod".to_string(), "ns".to_string(), sample_containers());
        sel.move_up();
        assert_eq!(sel.cursor, 0);
    }

    #[test]
    fn move_on_empty_does_nothing() {
        let mut sel = ContainerSelector::new();
        sel.move_up();
        sel.move_down();
        assert_eq!(sel.cursor, 0);
    }

    // -- Selection ----------------------------------------------------------

    #[test]
    fn confirm_selects_current_and_hides() {
        let mut sel = ContainerSelector::new();
        sel.open("pod".to_string(), "ns".to_string(), sample_containers());
        sel.cursor = 1;
        let ok = sel.confirm();
        assert!(ok);
        assert!(!sel.visible);
        assert_eq!(sel.selected, Some(1));
    }

    #[test]
    fn confirm_returns_false_on_empty() {
        let mut sel = ContainerSelector::new();
        sel.open("pod".to_string(), "ns".to_string(), vec![]);
        let ok = sel.confirm();
        assert!(!ok);
        assert!(sel.selected.is_none());
    }

    #[test]
    fn take_selection_returns_correct_data() {
        let mut sel = ContainerSelector::new();
        sel.open(
            "my-pod".to_string(),
            "kube-system".to_string(),
            sample_containers(),
        );
        sel.cursor = 1; // "sidecar"
        sel.confirm();
        let selection = sel.take_selection().expect("should have selection");
        assert_eq!(selection.pod_name, "my-pod");
        assert_eq!(selection.namespace, "kube-system");
        assert_eq!(selection.container_name, "sidecar");
        assert_eq!(selection.action, ContainerAction::Logs);
    }

    #[test]
    fn take_selection_with_shell_action() {
        let mut sel = ContainerSelector::new();
        sel.open_with_action(
            "pod".to_string(),
            "ns".to_string(),
            sample_containers(),
            ContainerAction::ShellExec,
        );
        sel.confirm();
        let selection = sel.take_selection().unwrap();
        assert_eq!(selection.action, ContainerAction::ShellExec);
    }

    #[test]
    fn take_selection_consumes() {
        let mut sel = ContainerSelector::new();
        sel.open(
            "pod".to_string(),
            "ns".to_string(),
            vec![ContainerInfo {
                name: "app".into(),
                is_init: false,
                status: "Running".into(),
                image: "app:v1".into(),
                restarts: 0,
            }],
        );
        sel.confirm();
        assert!(sel.take_selection().is_some());
        // Second take returns None
        assert!(sel.take_selection().is_none());
    }

    // -- container_count ---------------------------------------------------

    #[test]
    fn container_count_returns_length() {
        let mut sel = ContainerSelector::new();
        assert_eq!(sel.container_count(), 0);
        sel.open("pod".to_string(), "ns".to_string(), sample_containers());
        assert_eq!(sel.container_count(), 3);
    }

    // -- Reopen resets state -----------------------------------------------

    #[test]
    fn reopen_resets_cursor_and_selection() {
        let mut sel = ContainerSelector::new();
        sel.open("p1".to_string(), "ns1".to_string(), sample_containers());
        sel.move_down();
        sel.move_down();
        sel.confirm();

        sel.open("p2".to_string(), "ns2".to_string(), sample_containers());
        assert_eq!(sel.cursor, 0);
        assert!(sel.selected.is_none());
        assert_eq!(sel.pod_name, "p2");
        assert!(sel.visible);
    }

    // -- ContainerInfo construction ----------------------------------------

    #[test]
    fn container_info_init_vs_regular() {
        let init = ContainerInfo {
            name: "init-db".to_string(),
            is_init: true,
            status: "Completed".to_string(),
            image: "migrate:v1".to_string(),
            restarts: 0,
        };
        let regular = ContainerInfo {
            name: "app".to_string(),
            is_init: false,
            status: "Running".to_string(),
            image: "app:latest".to_string(),
            restarts: 3,
        };
        assert!(init.is_init);
        assert!(!regular.is_init);
        assert_eq!(regular.restarts, 3);
    }

    // -- extract_containers_from_value ------------------------------------

    #[test]
    fn extract_from_value_basic() {
        let pod_json = serde_json::json!({
            "spec": {
                "containers": [
                    {"name": "nginx", "image": "nginx:1.21"},
                    {"name": "sidecar", "image": "envoy:1.0"}
                ],
                "initContainers": [
                    {"name": "init-db", "image": "migrate:v1"}
                ]
            },
            "status": {
                "containerStatuses": [
                    {"name": "nginx", "restartCount": 0, "state": {"running": {}}},
                    {"name": "sidecar", "restartCount": 2, "state": {"running": {}}}
                ],
                "initContainerStatuses": [
                    {"name": "init-db", "restartCount": 0, "state": {"terminated": {"reason": "Completed"}}}
                ]
            }
        });

        let containers = extract_containers_from_value(&pod_json);
        assert_eq!(containers.len(), 3);

        // Init containers first
        assert_eq!(containers[0].name, "init-db");
        assert!(containers[0].is_init);
        assert_eq!(containers[0].status, "Completed");

        // Regular containers
        assert_eq!(containers[1].name, "nginx");
        assert!(!containers[1].is_init);
        assert_eq!(containers[1].status, "Running");

        assert_eq!(containers[2].name, "sidecar");
        assert!(!containers[2].is_init);
        assert_eq!(containers[2].restarts, 2);
    }

    #[test]
    fn extract_from_value_no_init_containers() {
        let pod_json = serde_json::json!({
            "spec": {
                "containers": [{"name": "app", "image": "app:v1"}]
            },
            "status": {
                "containerStatuses": [
                    {"name": "app", "restartCount": 0, "state": {"running": {}}}
                ]
            }
        });

        let containers = extract_containers_from_value(&pod_json);
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].name, "app");
        assert!(!containers[0].is_init);
    }

    #[test]
    fn extract_from_value_no_status() {
        let pod_json = serde_json::json!({
            "spec": {
                "containers": [{"name": "app", "image": "app:v1"}]
            }
        });
        let containers = extract_containers_from_value(&pod_json);
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].status, "Pending");
    }

    #[test]
    fn extract_from_value_empty_spec() {
        let pod_json = serde_json::json!({});
        let containers = extract_containers_from_value(&pod_json);
        assert!(containers.is_empty());
    }

    #[test]
    fn extract_from_value_waiting_state() {
        let pod_json = serde_json::json!({
            "spec": {
                "containers": [{"name": "app", "image": "app:v1"}]
            },
            "status": {
                "containerStatuses": [
                    {"name": "app", "restartCount": 5, "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
                ]
            }
        });
        let containers = extract_containers_from_value(&pod_json);
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].status, "CrashLoopBackOff");
        assert_eq!(containers[0].restarts, 5);
    }

    // -- container_status_from_value edge cases ----------------------------

    #[test]
    fn status_from_value_none() {
        let (status, restarts) = container_status_from_value(None);
        assert_eq!(status, "Pending");
        assert_eq!(restarts, 0);
    }

    #[test]
    fn status_from_value_running() {
        let v = serde_json::json!({"restartCount": 1, "state": {"running": {}}});
        let (status, restarts) = container_status_from_value(Some(&v));
        assert_eq!(status, "Running");
        assert_eq!(restarts, 1);
    }

    #[test]
    fn status_from_value_terminated_no_reason() {
        let v = serde_json::json!({"restartCount": 0, "state": {"terminated": {}}});
        let (status, _) = container_status_from_value(Some(&v));
        assert_eq!(status, "Terminated");
    }

    #[test]
    fn status_from_value_waiting_no_reason() {
        let v = serde_json::json!({"restartCount": 0, "state": {"waiting": {}}});
        let (status, _) = container_status_from_value(Some(&v));
        assert_eq!(status, "Waiting");
    }

    #[test]
    fn status_from_value_no_state() {
        let v = serde_json::json!({"restartCount": 3});
        let (status, restarts) = container_status_from_value(Some(&v));
        assert_eq!(status, "Unknown");
        assert_eq!(restarts, 3);
    }

    // -- ContainerSelection ------------------------------------------------

    #[test]
    fn container_selection_fields() {
        let s = ContainerSelection {
            pod_name: "pod-1".to_string(),
            namespace: "prod".to_string(),
            container_name: "web".to_string(),
            action: ContainerAction::Logs,
        };
        assert_eq!(s.pod_name, "pod-1");
        assert_eq!(s.namespace, "prod");
        assert_eq!(s.container_name, "web");
        assert_eq!(s.action, ContainerAction::Logs);
    }

    #[test]
    fn container_selection_with_shell_action() {
        let s = ContainerSelection {
            pod_name: "pod-2".to_string(),
            namespace: "staging".to_string(),
            container_name: "debug".to_string(),
            action: ContainerAction::ShellExec,
        };
        assert_eq!(s.action, ContainerAction::ShellExec);
    }
}
