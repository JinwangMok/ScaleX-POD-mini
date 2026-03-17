//! Port-forward manager — spawns and tracks `kubectl port-forward` style
//! subprocesses for active port-forward sessions.
//!
//! Each forward is represented as a background tokio task holding a child
//! process.  The manager provides add / stop / restart / list operations
//! and reports status changes via a channel so the TUI can display feedback.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::dash::port_picker::PortForwardSelection;

// ---------------------------------------------------------------------------
// ID generation
// ---------------------------------------------------------------------------

static NEXT_PF_ID: AtomicU32 = AtomicU32::new(1);

fn next_id() -> u32 {
    NEXT_PF_ID.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Status of a single port-forward session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PfStatus {
    /// Starting up (subprocess spawned but not yet confirmed listening).
    Starting,
    /// Actively forwarding traffic.
    Active,
    /// Failed with an error message.
    Failed(String),
    /// Stopped by user or subprocess exit.
    Stopped,
}

/// A tracked port-forward entry.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PortForwardEntry {
    pub id: u32,
    /// Resource being forwarded (e.g. "pod/nginx-abc123").
    pub resource: String,
    /// Namespace of the resource.
    pub namespace: String,
    /// Cluster context name.
    pub cluster: String,
    /// Remote (container) port.
    pub remote_port: u16,
    /// Local port bound on localhost.
    pub local_port: u16,
    /// Current status.
    pub status: PfStatus,
    /// When the forward was created.
    pub created_at: Instant,
    /// Kubeconfig path used for this forward.
    pub kubeconfig_path: Option<String>,
}

/// Events sent from background tasks back to the TUI.
#[derive(Debug)]
pub enum PfEvent {
    /// The port-forward became active (confirmed listening).
    Active { id: u32 },
    /// The port-forward failed.
    Failed { id: u32, error: String },
    /// The port-forward subprocess exited.
    Exited { id: u32, message: String },
}

// ---------------------------------------------------------------------------
// PortForwardManager
// ---------------------------------------------------------------------------

/// Manages active port-forward sessions.
///
/// Designed to be owned by `App`.  Background tasks communicate status
/// via `event_rx` which should be polled in the TUI event loop.
#[derive(Debug)]
pub struct PortForwardManager {
    /// Active forward entries keyed by id.
    entries: HashMap<u32, PortForwardEntry>,
    /// Ordered list of entry IDs (insertion order) for list display.
    order: Vec<u32>,
    /// Channel sender — cloned into each background task.
    event_tx: mpsc::Sender<PfEvent>,
    /// Channel receiver — polled in the TUI event loop.
    pub event_rx: mpsc::Receiver<PfEvent>,
    /// Abort handles for background tasks (keyed by id).
    abort_handles: HashMap<u32, tokio::task::AbortHandle>,
    /// Cursor position in the port-forward manager overlay list.
    pub cursor: usize,
    /// Whether the manager overlay is visible.
    pub visible: bool,
}

#[allow(dead_code)]
impl PortForwardManager {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel(64);
        Self {
            entries: HashMap::new(),
            order: Vec::new(),
            event_tx,
            event_rx,
            abort_handles: HashMap::new(),
            cursor: 0,
            visible: false,
        }
    }

    /// Start a new port-forward from a confirmed port-picker selection.
    ///
    /// Spawns a background task that runs `kubectl port-forward` via
    /// a child process. Returns the assigned ID.
    pub fn start(
        &mut self,
        selection: PortForwardSelection,
        cluster_name: &str,
        kubeconfig_path: Option<&str>,
    ) -> u32 {
        let id = next_id();

        let resource = format!("{}/{}", selection.resource_kind, selection.resource_name);
        let entry = PortForwardEntry {
            id,
            resource: resource.clone(),
            namespace: selection.namespace.clone(),
            cluster: cluster_name.to_string(),
            remote_port: selection.container_port,
            local_port: selection.local_port,
            status: PfStatus::Starting,
            created_at: Instant::now(),
            kubeconfig_path: kubeconfig_path.map(|s| s.to_string()),
        };

        self.entries.insert(id, entry);
        self.order.push(id);

        // Spawn background subprocess task
        let tx = self.event_tx.clone();
        let ns = selection.namespace.clone();
        let port_mapping = format!("{}:{}", selection.local_port, selection.container_port);
        let kc = kubeconfig_path.map(|s| s.to_string());

        let handle = tokio::spawn(async move {
            Self::run_port_forward(tx, id, &resource, &ns, &port_mapping, kc.as_deref()).await;
        });

        self.abort_handles.insert(id, handle.abort_handle());
        id
    }

    /// Background task: spawn kubectl port-forward and monitor its output.
    async fn run_port_forward(
        tx: mpsc::Sender<PfEvent>,
        id: u32,
        resource: &str,
        namespace: &str,
        port_mapping: &str,
        kubeconfig: Option<&str>,
    ) {
        let mut cmd = Command::new("kubectl");
        cmd.arg("port-forward")
            .arg("-n")
            .arg(namespace)
            .arg(resource)
            .arg(port_mapping)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(kc) = kubeconfig {
            cmd.arg("--kubeconfig").arg(kc);
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(PfEvent::Failed {
                        id,
                        error: format!("Failed to spawn kubectl: {}", e),
                    })
                    .await;
                return;
            }
        };

        // Wait briefly for the process to start and check for immediate failure
        let output = match child.wait_with_output().await {
            Ok(o) => o,
            Err(e) => {
                let _ = tx
                    .send(PfEvent::Failed {
                        id,
                        error: format!("kubectl port-forward error: {}", e),
                    })
                    .await;
                return;
            }
        };

        if output.status.success() {
            let _ = tx
                .send(PfEvent::Exited {
                    id,
                    message: "Port-forward exited normally".to_string(),
                })
                .await;
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = if stderr.is_empty() {
                format!("kubectl port-forward exited with code {:?}", output.status.code())
            } else {
                stderr.trim().to_string()
            };
            let _ = tx.send(PfEvent::Failed { id, error: msg }).await;
        }
    }

    /// Alternative: spawn port-forward as a long-running process that we monitor line-by-line.
    /// This version reads stderr line by line so we can detect "Forwarding from" messages.
    pub fn start_monitored(
        &mut self,
        selection: PortForwardSelection,
        cluster_name: &str,
        kubeconfig_path: Option<&str>,
    ) -> u32 {
        let id = next_id();

        let resource = format!("{}/{}", selection.resource_kind, selection.resource_name);
        let entry = PortForwardEntry {
            id,
            resource: resource.clone(),
            namespace: selection.namespace.clone(),
            cluster: cluster_name.to_string(),
            remote_port: selection.container_port,
            local_port: selection.local_port,
            status: PfStatus::Starting,
            created_at: Instant::now(),
            kubeconfig_path: kubeconfig_path.map(|s| s.to_string()),
        };

        self.entries.insert(id, entry);
        self.order.push(id);

        let tx = self.event_tx.clone();
        let ns = selection.namespace.clone();
        let port_mapping = format!("{}:{}", selection.local_port, selection.container_port);
        let kc = kubeconfig_path.map(|s| s.to_string());

        let handle = tokio::spawn(async move {
            Self::run_port_forward_monitored(tx, id, &resource, &ns, &port_mapping, kc.as_deref())
                .await;
        });

        self.abort_handles.insert(id, handle.abort_handle());
        id
    }

    /// Monitored version: reads stdout/stderr line by line to detect readiness.
    async fn run_port_forward_monitored(
        tx: mpsc::Sender<PfEvent>,
        id: u32,
        resource: &str,
        namespace: &str,
        port_mapping: &str,
        kubeconfig: Option<&str>,
    ) {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut cmd = Command::new("kubectl");
        cmd.arg("port-forward")
            .arg("-n")
            .arg(namespace)
            .arg(resource)
            .arg(port_mapping)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(kc) = kubeconfig {
            cmd.arg("--kubeconfig").arg(kc);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(PfEvent::Failed {
                        id,
                        error: format!("Failed to spawn kubectl: {}", e),
                    })
                    .await;
                return;
            }
        };

        // Read stdout for "Forwarding from" confirmation
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let mut active_sent = false;

        // Monitor stdout (kubectl port-forward prints "Forwarding from..." to stdout)
        if let Some(out) = stdout {
            let mut reader = BufReader::new(out).lines();
            // Also spawn stderr reader in background
            let tx_err = tx.clone();
            let stderr_task = if let Some(err) = stderr {
                Some(tokio::spawn(async move {
                    let mut reader = BufReader::new(err).lines();
                    let mut collected = String::new();
                    while let Ok(Some(line)) = reader.next_line().await {
                        if !collected.is_empty() {
                            collected.push('\n');
                        }
                        collected.push_str(&line);
                    }
                    collected
                }))
            } else {
                let _ = tx_err; // suppress unused
                None
            };

            // Read stdout lines - look for "Forwarding from" to confirm active
            while let Ok(Some(line)) = reader.next_line().await {
                if !active_sent && line.contains("Forwarding from") {
                    active_sent = true;
                    let _ = tx.send(PfEvent::Active { id }).await;
                }
            }

            // Process exited — wait for status
            let status = child.wait().await;
            let stderr_output = if let Some(task) = stderr_task {
                task.await.unwrap_or_default()
            } else {
                String::new()
            };

            match status {
                Ok(s) if s.success() => {
                    let _ = tx
                        .send(PfEvent::Exited {
                            id,
                            message: "Port-forward closed".to_string(),
                        })
                        .await;
                }
                Ok(s) => {
                    let msg = if stderr_output.is_empty() {
                        format!("Exited with code {:?}", s.code())
                    } else {
                        stderr_output.trim().to_string()
                    };
                    let _ = tx.send(PfEvent::Failed { id, error: msg }).await;
                }
                Err(e) => {
                    let _ = tx
                        .send(PfEvent::Failed {
                            id,
                            error: format!("Wait failed: {}", e),
                        })
                        .await;
                }
            }
        } else {
            // No stdout pipe — just wait for exit
            match child.wait().await {
                Ok(s) if s.success() => {
                    let _ = tx
                        .send(PfEvent::Exited {
                            id,
                            message: "Port-forward closed".to_string(),
                        })
                        .await;
                }
                Ok(_) | Err(_) => {
                    let _ = tx
                        .send(PfEvent::Failed {
                            id,
                            error: "Port-forward exited unexpectedly".to_string(),
                        })
                        .await;
                }
            }
        }
    }

    /// Stop a port-forward by ID. Aborts the background task (which kills the subprocess).
    pub fn stop(&mut self, id: u32) {
        if let Some(handle) = self.abort_handles.remove(&id) {
            handle.abort();
        }
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.status = PfStatus::Stopped;
        }
    }

    /// Stop all active port-forwards. Called on TUI exit.
    pub fn stop_all(&mut self) {
        let ids: Vec<u32> = self.order.clone();
        for id in ids {
            self.stop(id);
        }
    }

    /// Remove a stopped/failed entry from the list.
    pub fn remove(&mut self, id: u32) {
        self.stop(id); // ensure subprocess is killed
        self.entries.remove(&id);
        self.order.retain(|&i| i != id);
        // Clamp cursor
        if !self.order.is_empty() && self.cursor >= self.order.len() {
            self.cursor = self.order.len() - 1;
        }
    }

    /// Process a status event from the background task channel.
    pub fn handle_event(&mut self, event: PfEvent) {
        match event {
            PfEvent::Active { id } => {
                if let Some(entry) = self.entries.get_mut(&id) {
                    entry.status = PfStatus::Active;
                }
            }
            PfEvent::Failed { id, ref error } => {
                if let Some(entry) = self.entries.get_mut(&id) {
                    entry.status = PfStatus::Failed(error.clone());
                }
                // Clean up abort handle
                self.abort_handles.remove(&id);
            }
            PfEvent::Exited { id, .. } => {
                if let Some(entry) = self.entries.get_mut(&id) {
                    entry.status = PfStatus::Stopped;
                }
                self.abort_handles.remove(&id);
            }
        }
    }

    /// Get the currently selected entry (based on cursor).
    pub fn selected_entry(&self) -> Option<&PortForwardEntry> {
        self.order
            .get(self.cursor)
            .and_then(|id| self.entries.get(id))
    }

    /// Get the ID of the selected entry.
    pub fn selected_id(&self) -> Option<u32> {
        self.order.get(self.cursor).copied()
    }

    /// Get ordered list of all entries.
    pub fn entries(&self) -> Vec<&PortForwardEntry> {
        self.order
            .iter()
            .filter_map(|id| self.entries.get(id))
            .collect()
    }

    /// Number of active entries.
    pub fn count(&self) -> usize {
        self.order.len()
    }

    /// Number of actively forwarding entries.
    pub fn active_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.status == PfStatus::Active)
            .count()
    }

    /// Check if there's already a forward for this local port.
    pub fn is_port_in_use(&self, local_port: u16) -> bool {
        self.entries.values().any(|e| {
            e.local_port == local_port
                && matches!(e.status, PfStatus::Active | PfStatus::Starting)
        })
    }

    // -- Overlay navigation --

    pub fn open(&mut self) {
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.order.is_empty() && self.cursor < self.order.len() - 1 {
            self.cursor += 1;
        }
    }

    /// Delete (stop) the selected entry.
    pub fn delete_selected(&mut self) {
        if let Some(id) = self.selected_id() {
            self.remove(id);
        }
    }

    // -- Rendering --

    /// Render the port-forward manager overlay.
    pub fn render(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        use crate::dash::theme;
        use ratatui::layout::Rect;
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Clear, Paragraph};

        if !self.visible {
            return;
        }

        let entries = self.entries();
        let list_rows = entries.len().max(1) as u16;
        let popup_height = (list_rows + 6)
            .min(24)
            .min(area.height.saturating_sub(4));
        let popup_width = 80u16.min(area.width.saturating_sub(4)).max(50);

        if popup_height < 5 || popup_width < 40 {
            return;
        }

        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        // Clear background
        f.render_widget(Clear, popup_area);

        let active = self.active_count();
        let total = self.count();
        let title = format!(" Port Forwards ({}/{} active) ", active, total);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::BRIGHT_ORANGE))
            .style(Style::default().bg(theme::BG));
        f.render_widget(block, popup_area);

        let inner = Rect::new(
            popup_area.x + 1,
            popup_area.y + 1,
            popup_area.width.saturating_sub(2),
            popup_area.height.saturating_sub(2),
        );

        let sep_len = inner.width.saturating_sub(2) as usize;
        let mut lines: Vec<Line<'_>> = Vec::new();

        // Header
        lines.push(Line::from(vec![Span::styled(
            format!(
                " {:3} {:20} {:12} {:8} {:10}",
                "ID", "RESOURCE", "LOCAL:REMOTE", "STATUS", "CLUSTER"
            ),
            Style::default()
                .fg(theme::BRIGHT_ORANGE)
                .add_modifier(Modifier::BOLD),
        )]));

        lines.push(Line::from(Span::styled(
            format!(" {}", "\u{2500}".repeat(sep_len)),
            Style::default().fg(theme::BG3),
        )));

        if entries.is_empty() {
            lines.push(Line::from(Span::styled(
                " (no active port-forwards — press Shift+F on a pod/svc to start)",
                Style::default().fg(theme::FG4),
            )));
        } else {
            for (i, entry) in entries.iter().enumerate() {
                let is_selected = i == self.cursor;
                let (indicator, style) = if is_selected {
                    (
                        "\u{25b8}",
                        Style::default()
                            .fg(theme::BRIGHT_AQUA)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    (" ", Style::default().fg(theme::FG))
                };

                let status_str = match &entry.status {
                    PfStatus::Starting => "starting",
                    PfStatus::Active => "active",
                    PfStatus::Failed(_) => "failed",
                    PfStatus::Stopped => "stopped",
                };

                let status_color = match &entry.status {
                    PfStatus::Starting => theme::BRIGHT_YELLOW,
                    PfStatus::Active => theme::BRIGHT_GREEN,
                    PfStatus::Failed(_) => theme::BRIGHT_RED,
                    PfStatus::Stopped => theme::FG4,
                };

                let port_mapping = format!("{}:{}", entry.local_port, entry.remote_port);
                let resource_display = if entry.resource.len() > 20 {
                    format!("{}…", &entry.resource[..19])
                } else {
                    entry.resource.clone()
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", indicator), style),
                    Span::styled(format!("{:<3} ", entry.id), style),
                    Span::styled(format!("{:<20} ", resource_display), style),
                    Span::styled(format!("{:<12} ", port_mapping), style),
                    Span::styled(
                        format!("{:<8} ", status_str),
                        Style::default().fg(status_color),
                    ),
                    Span::styled(entry.cluster.clone(), Style::default().fg(theme::FG4)),
                ]));
            }
        }

        // Bottom separator + hints
        lines.push(Line::from(Span::styled(
            format!(" {}", "\u{2500}".repeat(sep_len)),
            Style::default().fg(theme::BG3),
        )));
        lines.push(Line::from(vec![Span::styled(
            " j/k: navigate  d: stop/remove  ESC: close",
            Style::default().fg(theme::FG4),
        )]));

        let paragraph = Paragraph::new(lines).style(Style::default().bg(theme::BG));
        f.render_widget(paragraph, inner);
    }
}

impl Default for PortForwardManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::port_picker::PortForwardSelection;

    fn make_selection(local: u16, remote: u16) -> PortForwardSelection {
        PortForwardSelection {
            resource_name: "nginx-abc".to_string(),
            namespace: "default".to_string(),
            resource_kind: "pod".to_string(),
            container_port: remote,
            local_port: local,
            protocol: "TCP".to_string(),
        }
    }

    #[test]
    fn new_manager_is_empty() {
        let mgr = PortForwardManager::new();
        assert_eq!(mgr.count(), 0);
        assert_eq!(mgr.active_count(), 0);
        assert!(!mgr.visible);
    }

    #[test]
    fn port_in_use_check() {
        let mut mgr = PortForwardManager::new();
        assert!(!mgr.is_port_in_use(8080));

        // Manually insert an entry to test (skip subprocess spawn)
        let entry = PortForwardEntry {
            id: 1,
            resource: "pod/nginx".to_string(),
            namespace: "default".to_string(),
            cluster: "test".to_string(),
            remote_port: 80,
            local_port: 8080,
            status: PfStatus::Active,
            created_at: Instant::now(),
            kubeconfig_path: None,
        };
        mgr.entries.insert(1, entry);
        mgr.order.push(1);

        assert!(mgr.is_port_in_use(8080));
        assert!(!mgr.is_port_in_use(9090));
    }

    #[test]
    fn stop_changes_status() {
        let mut mgr = PortForwardManager::new();
        let entry = PortForwardEntry {
            id: 1,
            resource: "pod/nginx".to_string(),
            namespace: "default".to_string(),
            cluster: "test".to_string(),
            remote_port: 80,
            local_port: 8080,
            status: PfStatus::Active,
            created_at: Instant::now(),
            kubeconfig_path: None,
        };
        mgr.entries.insert(1, entry);
        mgr.order.push(1);

        mgr.stop(1);
        assert_eq!(mgr.entries.get(&1).unwrap().status, PfStatus::Stopped);
    }

    #[test]
    fn remove_cleans_up() {
        let mut mgr = PortForwardManager::new();
        let entry = PortForwardEntry {
            id: 1,
            resource: "pod/nginx".to_string(),
            namespace: "default".to_string(),
            cluster: "test".to_string(),
            remote_port: 80,
            local_port: 8080,
            status: PfStatus::Stopped,
            created_at: Instant::now(),
            kubeconfig_path: None,
        };
        mgr.entries.insert(1, entry);
        mgr.order.push(1);

        mgr.remove(1);
        assert_eq!(mgr.count(), 0);
        assert!(mgr.entries.is_empty());
    }

    #[test]
    fn handle_active_event() {
        let mut mgr = PortForwardManager::new();
        let entry = PortForwardEntry {
            id: 1,
            resource: "pod/nginx".to_string(),
            namespace: "default".to_string(),
            cluster: "test".to_string(),
            remote_port: 80,
            local_port: 8080,
            status: PfStatus::Starting,
            created_at: Instant::now(),
            kubeconfig_path: None,
        };
        mgr.entries.insert(1, entry);
        mgr.order.push(1);

        mgr.handle_event(PfEvent::Active { id: 1 });
        assert_eq!(mgr.entries.get(&1).unwrap().status, PfStatus::Active);
    }

    #[test]
    fn handle_failed_event() {
        let mut mgr = PortForwardManager::new();
        let entry = PortForwardEntry {
            id: 1,
            resource: "pod/nginx".to_string(),
            namespace: "default".to_string(),
            cluster: "test".to_string(),
            remote_port: 80,
            local_port: 8080,
            status: PfStatus::Starting,
            created_at: Instant::now(),
            kubeconfig_path: None,
        };
        mgr.entries.insert(1, entry);
        mgr.order.push(1);

        mgr.handle_event(PfEvent::Failed {
            id: 1,
            error: "connection refused".to_string(),
        });
        assert!(matches!(
            mgr.entries.get(&1).unwrap().status,
            PfStatus::Failed(_)
        ));
    }

    #[test]
    fn overlay_navigation() {
        let mut mgr = PortForwardManager::new();
        for i in 0..3 {
            let entry = PortForwardEntry {
                id: i,
                resource: format!("pod/nginx-{}", i),
                namespace: "default".to_string(),
                cluster: "test".to_string(),
                remote_port: 80,
                local_port: 8080 + i as u16,
                status: PfStatus::Active,
                created_at: Instant::now(),
                kubeconfig_path: None,
            };
            mgr.entries.insert(i, entry);
            mgr.order.push(i);
        }

        assert_eq!(mgr.cursor, 0);
        mgr.move_down();
        assert_eq!(mgr.cursor, 1);
        mgr.move_down();
        assert_eq!(mgr.cursor, 2);
        mgr.move_down(); // at end, no-op
        assert_eq!(mgr.cursor, 2);
        mgr.move_up();
        assert_eq!(mgr.cursor, 1);
    }

    #[test]
    fn entries_preserves_order() {
        let mut mgr = PortForwardManager::new();
        for i in [3u32, 1, 2] {
            let entry = PortForwardEntry {
                id: i,
                resource: format!("pod/nginx-{}", i),
                namespace: "default".to_string(),
                cluster: "test".to_string(),
                remote_port: 80,
                local_port: 8080 + i as u16,
                status: PfStatus::Active,
                created_at: Instant::now(),
                kubeconfig_path: None,
            };
            mgr.entries.insert(i, entry);
            mgr.order.push(i);
        }

        let entries = mgr.entries();
        assert_eq!(entries[0].id, 3);
        assert_eq!(entries[1].id, 1);
        assert_eq!(entries[2].id, 2);
    }

    #[test]
    fn selection_fields_roundtrip() {
        let sel = make_selection(9090, 80);
        assert_eq!(sel.local_port, 9090);
        assert_eq!(sel.container_port, 80);
        assert_eq!(sel.resource_kind, "pod");
    }

    #[test]
    fn stop_all_stops_everything() {
        let mut mgr = PortForwardManager::new();
        for i in 0..3 {
            let entry = PortForwardEntry {
                id: i,
                resource: format!("pod/nginx-{}", i),
                namespace: "default".to_string(),
                cluster: "test".to_string(),
                remote_port: 80,
                local_port: 8080 + i as u16,
                status: PfStatus::Active,
                created_at: Instant::now(),
                kubeconfig_path: None,
            };
            mgr.entries.insert(i, entry);
            mgr.order.push(i);
        }

        mgr.stop_all();
        for entry in mgr.entries.values() {
            assert_eq!(entry.status, PfStatus::Stopped);
        }
    }
}
