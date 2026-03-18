//! Port-picker modal widget — displays discovered container ports in a
//! selectable list with an editable local-port override field.
//!
//! Used before initiating a `kubectl port-forward`-style tunnel via kube-rs.
//! The user selects a container port (or enters a custom one), optionally
//! overrides the local port, and confirms to start the forward.
//!
//! # Key bindings (when port picker is visible)
//!
//! | Key           | Action                                  |
//! |---------------|-----------------------------------------|
//! | `j`/`↓`       | Move cursor down in port list           |
//! | `k`/`↑`       | Move cursor up in port list             |
//! | `Tab`         | Toggle focus: port list ↔ local port    |
//! | `0-9`         | Edit local port override (when focused) |
//! | `Backspace`   | Delete char in local port field         |
//! | `Enter`       | Confirm and start port-forward          |
//! | `ESC`/`q`     | Cancel and close                        |

use crate::dash::theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// PortInfo — describes a single discovered port
// ---------------------------------------------------------------------------

/// Metadata for a container port shown in the picker list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// Container port number.
    pub container_port: u16,
    /// Protocol (TCP, UDP, SCTP). Defaults to "TCP" if unset.
    pub protocol: String,
    /// Optional port name from the container spec (e.g. "http", "grpc").
    pub name: String,
    /// Container that exposes this port.
    pub container_name: String,
}

// ---------------------------------------------------------------------------
// PortForwardSelection — the result of a port-forward pick
// ---------------------------------------------------------------------------

/// Result from the port picker, consumed by the port-forward handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForwardSelection {
    /// Pod (or service) name.
    pub resource_name: String,
    /// Namespace.
    pub namespace: String,
    /// Resource kind (e.g. "pod", "svc").
    pub resource_kind: String,
    /// Container port to forward.
    pub container_port: u16,
    /// Local port to bind (may differ from container port).
    pub local_port: u16,
    /// Protocol for the forward.
    pub protocol: String,
}

// ---------------------------------------------------------------------------
// Focus — which field has keyboard focus
// ---------------------------------------------------------------------------

/// Which sub-element of the port picker has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerFocus {
    /// The port list (j/k to navigate).
    PortList,
    /// The local-port override text field.
    LocalPort,
}

// ---------------------------------------------------------------------------
// PortPicker state
// ---------------------------------------------------------------------------

/// State for the port-picker modal.
///
/// When `visible` is true, this modal intercepts keyboard events and renders
/// a centered popup with a selectable port list and a local-port override
/// field. Confirming the selection closes the modal and returns the result
/// for the caller to initiate the port-forward.
#[derive(Debug, Clone)]
pub struct PortPicker {
    /// Whether the modal is currently shown.
    pub visible: bool,
    /// Resource name (pod or service).
    pub resource_name: String,
    /// Namespace.
    pub namespace: String,
    /// Resource kind ("pod", "svc", etc.).
    pub resource_kind: String,
    /// Discovered ports from the resource spec.
    pub ports: Vec<PortInfo>,
    /// Current cursor position in the port list.
    pub cursor: usize,
    /// Which field has keyboard focus.
    pub focus: PickerFocus,
    /// Editable local-port override text. Empty means "same as container port".
    pub local_port_input: String,
    /// If set, the user confirmed a selection. Consumed by the caller.
    pub confirmed: Option<PortForwardSelection>,
}

impl PortPicker {
    /// Create a new hidden port picker.
    pub fn new() -> Self {
        Self {
            visible: false,
            resource_name: String::new(),
            namespace: String::new(),
            resource_kind: String::new(),
            ports: Vec::new(),
            cursor: 0,
            focus: PickerFocus::PortList,
            local_port_input: String::new(),
            confirmed: None,
        }
    }

    /// Open the port picker for a resource.
    ///
    /// `ports` should contain the discovered container ports. If empty, the
    /// picker still opens to allow manual entry of a local port (the user
    /// might know the port from the service spec or docs).
    pub fn open(
        &mut self,
        resource_name: String,
        namespace: String,
        resource_kind: String,
        ports: Vec<PortInfo>,
    ) {
        self.resource_name = resource_name;
        self.namespace = namespace;
        self.resource_kind = resource_kind;
        self.ports = ports;
        self.cursor = 0;
        self.focus = PickerFocus::PortList;
        self.local_port_input.clear();
        self.confirmed = None;
        self.visible = true;

        // Pre-fill local port from first port if available
        if let Some(first) = self.ports.first() {
            self.local_port_input = first.container_port.to_string();
        }
    }

    /// Close the picker without confirming.
    pub fn close(&mut self) {
        self.visible = false;
        self.ports.clear();
        self.cursor = 0;
        self.local_port_input.clear();
        self.confirmed = None;
    }

    // -- Navigation ---------------------------------------------------------

    /// Move cursor up in the port list.
    pub fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.sync_local_port_from_cursor();
        }
    }

    /// Move cursor down in the port list.
    pub fn move_down(&mut self) {
        if !self.ports.is_empty() && self.cursor < self.ports.len() - 1 {
            self.cursor += 1;
            self.sync_local_port_from_cursor();
        }
    }

    /// Toggle focus between port list and local-port field.
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            PickerFocus::PortList => PickerFocus::LocalPort,
            PickerFocus::LocalPort => PickerFocus::PortList,
        };
    }

    // -- Local port editing -------------------------------------------------

    /// Append a digit character to the local-port input field.
    /// Only digits are accepted; the field is capped at 5 chars (max port 65535).
    pub fn type_digit(&mut self, ch: char) {
        if ch.is_ascii_digit() && self.local_port_input.len() < 5 {
            self.local_port_input.push(ch);
        }
    }

    /// Delete the last character from the local-port input.
    pub fn backspace(&mut self) {
        self.local_port_input.pop();
    }

    /// Clear the entire local-port input.
    pub fn clear_local_port(&mut self) {
        self.local_port_input.clear();
    }

    // -- Selection / confirmation -------------------------------------------

    /// The currently selected container port (from the list), or 0 if empty.
    pub fn selected_container_port(&self) -> u16 {
        self.ports
            .get(self.cursor)
            .map(|p| p.container_port)
            .unwrap_or(0)
    }

    /// The effective local port: parsed from the input field, or falls back
    /// to the selected container port.
    pub fn effective_local_port(&self) -> u16 {
        self.parse_local_port()
            .unwrap_or_else(|| self.selected_container_port())
    }

    /// Parse the local-port input field. Returns `None` if empty or invalid.
    pub fn parse_local_port(&self) -> Option<u16> {
        if self.local_port_input.is_empty() {
            return None;
        }
        self.local_port_input.parse::<u16>().ok().filter(|&p| p > 0)
    }

    /// The protocol of the currently selected port, or "TCP" as default.
    pub fn selected_protocol(&self) -> String {
        self.ports
            .get(self.cursor)
            .map(|p| p.protocol.clone())
            .unwrap_or_else(|| "TCP".to_string())
    }

    /// Confirm the current selection. Returns `true` if a valid selection
    /// was made, `false` if no ports and no valid local port input.
    pub fn confirm(&mut self) -> bool {
        let container_port = self.selected_container_port();
        let local_port = self.effective_local_port();

        // Must have a valid local port to forward
        if local_port == 0 {
            return false;
        }

        // If no ports discovered, container_port defaults to local_port
        let actual_container_port = if container_port == 0 {
            local_port
        } else {
            container_port
        };

        self.confirmed = Some(PortForwardSelection {
            resource_name: self.resource_name.clone(),
            namespace: self.namespace.clone(),
            resource_kind: self.resource_kind.clone(),
            container_port: actual_container_port,
            local_port,
            protocol: self.selected_protocol(),
        });
        self.visible = false;
        true
    }

    /// Take the pending confirmed selection (if any), consuming it.
    pub fn take_selection(&mut self) -> Option<PortForwardSelection> {
        self.confirmed.take()
    }

    /// Returns the number of discovered ports.
    pub fn port_count(&self) -> usize {
        self.ports.len()
    }

    /// Returns whether the local-port input field has a valid port number.
    pub fn is_local_port_valid(&self) -> bool {
        match self.parse_local_port() {
            Some(p) => p > 0,
            None => {
                // Empty input is valid (means "same as container port") if ports exist
                self.local_port_input.is_empty() && !self.ports.is_empty()
            }
        }
    }

    // -- Internal helpers ---------------------------------------------------

    /// Sync the local port input to match the currently selected port in the
    /// list (auto-fill behavior when navigating the list).
    fn sync_local_port_from_cursor(&mut self) {
        if let Some(port) = self.ports.get(self.cursor) {
            self.local_port_input = port.container_port.to_string();
        }
    }

    // -- Rendering ----------------------------------------------------------

    /// Render the port-picker modal overlay.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        if !self.visible {
            return;
        }

        // Compute popup dimensions
        let list_rows = self.ports.len().max(1) as u16;
        // header(1) + separator(1) + list + separator(1) + local_port_row(1) + separator(1) + hints(1) = 6 + list
        let popup_height = (list_rows + 8).min(24).min(area.height.saturating_sub(4));
        let popup_width = 72u16.min(area.width.saturating_sub(4)).max(44);

        if popup_height < 7 || popup_width < 34 {
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
            " Port Forward \u{2014} {}/{} ",
            self.resource_kind, self.resource_name
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::BRIGHT_ORANGE))
            .style(Style::default().bg(theme::BG));
        f.render_widget(block, popup_area);

        // Inner content area
        let inner = Rect::new(
            popup_area.x + 1,
            popup_area.y + 1,
            popup_area.width.saturating_sub(2),
            popup_area.height.saturating_sub(2),
        );

        let sep_len = inner.width.saturating_sub(2) as usize;
        let mut lines: Vec<Line<'_>> = Vec::new();

        // Column header
        lines.push(Line::from(vec![Span::styled(
            format!(" {:6} {:8} {:16} {}", "PORT", "PROTO", "NAME", "CONTAINER"),
            Style::default()
                .fg(theme::BRIGHT_ORANGE)
                .add_modifier(Modifier::BOLD),
        )]));

        // Separator
        lines.push(Line::from(Span::styled(
            format!(" {}", "\u{2500}".repeat(sep_len)),
            Style::default().fg(theme::BG3),
        )));

        // Port rows
        if self.ports.is_empty() {
            lines.push(Line::from(Span::styled(
                " (no ports discovered — enter local port manually)",
                Style::default().fg(theme::FG4),
            )));
        } else {
            for (i, port) in self.ports.iter().enumerate() {
                let is_selected = i == self.cursor;
                let list_focused = self.focus == PickerFocus::PortList;

                let (indicator, base_style) = if is_selected && list_focused {
                    (
                        "\u{25b8}",
                        Style::default()
                            .fg(theme::BRIGHT_AQUA)
                            .add_modifier(Modifier::BOLD),
                    )
                } else if is_selected {
                    ("\u{25b8}", Style::default().fg(theme::FG2))
                } else {
                    (" ", Style::default().fg(theme::FG))
                };

                let port_name = if port.name.is_empty() {
                    "-"
                } else {
                    &port.name
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", indicator), base_style),
                    Span::styled(format!("{:<6}", port.container_port), base_style),
                    Span::styled(
                        format!("{:<8}", port.protocol),
                        if is_selected {
                            base_style
                        } else {
                            Style::default().fg(theme::FG4)
                        },
                    ),
                    Span::styled(
                        format!("{:<16}", port_name),
                        if is_selected {
                            base_style
                        } else {
                            Style::default().fg(theme::FG4)
                        },
                    ),
                    Span::styled(
                        port.container_name.clone(),
                        if is_selected {
                            base_style
                        } else {
                            Style::default().fg(theme::FG4)
                        },
                    ),
                ]));
            }
        }

        // Separator before local port field
        lines.push(Line::from(Span::styled(
            format!(" {}", "\u{2500}".repeat(sep_len)),
            Style::default().fg(theme::BG3),
        )));

        // Local port override field
        let local_focused = self.focus == PickerFocus::LocalPort;
        let field_style = if local_focused {
            Style::default()
                .fg(theme::BRIGHT_AQUA)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::FG)
        };

        let port_display = if self.local_port_input.is_empty() {
            let fallback = self.selected_container_port();
            if fallback > 0 {
                format!("{} (auto)", fallback)
            } else {
                "0".to_string()
            }
        } else {
            self.local_port_input.clone()
        };

        let cursor_char = if local_focused { "\u{2588}" } else { "" };
        let validity = if self.effective_local_port() > 0 {
            Span::styled(" \u{2713}", Style::default().fg(theme::BRIGHT_GREEN))
        } else {
            Span::styled(" \u{2717}", Style::default().fg(theme::BRIGHT_RED))
        };

        lines.push(Line::from(vec![
            Span::styled(
                " Local port: ",
                if local_focused {
                    Style::default()
                        .fg(theme::BRIGHT_ORANGE)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::FG4)
                },
            ),
            Span::styled(port_display, field_style),
            Span::styled(
                cursor_char.to_string(),
                Style::default().fg(theme::BRIGHT_AQUA),
            ),
            validity,
        ]));

        // Bottom separator + hints
        lines.push(Line::from(Span::styled(
            format!(" {}", "\u{2500}".repeat(sep_len)),
            Style::default().fg(theme::BG3),
        )));
        lines.push(Line::from(vec![Span::styled(
            " \u{2191}\u{2193}/jk: select  Tab: switch field  Enter: confirm  ESC: cancel",
            Style::default().fg(theme::FG4),
        )]));

        let paragraph = Paragraph::new(lines).style(Style::default().bg(theme::BG));
        f.render_widget(paragraph, inner);
    }
}

impl Default for PortPicker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// DimOverlay — simple background dimmer (same pattern as container_selector)
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
// Helper: extract ports from a Pod or Service JSON value
// ---------------------------------------------------------------------------

/// Extract port info from a Pod's JSON representation (DynamicObject data).
///
/// Iterates all containers and their `ports` arrays.
pub fn extract_ports_from_pod(obj: &serde_json::Value) -> Vec<PortInfo> {
    let mut result = Vec::new();

    let spec = match obj.get("spec") {
        Some(s) => s,
        None => return result,
    };

    // Regular containers
    if let Some(containers) = spec.get("containers").and_then(|v| v.as_array()) {
        for c in containers {
            let container_name = c
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if let Some(ports) = c.get("ports").and_then(|v| v.as_array()) {
                for p in ports {
                    let container_port =
                        p.get("containerPort").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                    if container_port == 0 {
                        continue;
                    }
                    let protocol = p
                        .get("protocol")
                        .and_then(|v| v.as_str())
                        .unwrap_or("TCP")
                        .to_string();
                    let name = p
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    result.push(PortInfo {
                        container_port,
                        protocol,
                        name,
                        container_name: container_name.clone(),
                    });
                }
            }
        }
    }

    result
}

/// Extract port info from a Service's JSON representation (DynamicObject data).
///
/// Iterates the `spec.ports` array, using `targetPort` for the container port
/// and `port` as the service port.
pub fn extract_ports_from_service(obj: &serde_json::Value) -> Vec<PortInfo> {
    let mut result = Vec::new();

    let spec = match obj.get("spec") {
        Some(s) => s,
        None => return result,
    };

    if let Some(ports) = spec.get("ports").and_then(|v| v.as_array()) {
        for p in ports {
            // For services, "port" is the service port (what we forward to),
            // "targetPort" is the container port.
            let svc_port = p.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            let target_port = p
                .get("targetPort")
                .and_then(|v| v.as_u64())
                .unwrap_or(svc_port as u64) as u16;
            if svc_port == 0 {
                continue;
            }
            let protocol = p
                .get("protocol")
                .and_then(|v| v.as_str())
                .unwrap_or("TCP")
                .to_string();
            let name = p
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            result.push(PortInfo {
                container_port: svc_port,
                protocol,
                name,
                container_name: format!("target:{}", target_port),
            });
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ports() -> Vec<PortInfo> {
        vec![
            PortInfo {
                container_port: 8080,
                protocol: "TCP".to_string(),
                name: "http".to_string(),
                container_name: "web".to_string(),
            },
            PortInfo {
                container_port: 9090,
                protocol: "TCP".to_string(),
                name: "metrics".to_string(),
                container_name: "web".to_string(),
            },
            PortInfo {
                container_port: 5432,
                protocol: "TCP".to_string(),
                name: "postgres".to_string(),
                container_name: "db".to_string(),
            },
        ]
    }

    // -- PortPicker construction / state -----------------------------------

    #[test]
    fn new_picker_is_hidden() {
        let pp = PortPicker::new();
        assert!(!pp.visible);
        assert!(pp.ports.is_empty());
        assert_eq!(pp.cursor, 0);
        assert_eq!(pp.focus, PickerFocus::PortList);
        assert!(pp.local_port_input.is_empty());
        assert!(pp.confirmed.is_none());
    }

    #[test]
    fn default_is_same_as_new() {
        let pp = PortPicker::default();
        assert!(!pp.visible);
        assert!(pp.ports.is_empty());
    }

    #[test]
    fn open_sets_state() {
        let mut pp = PortPicker::new();
        pp.open(
            "my-pod".to_string(),
            "default".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        assert!(pp.visible);
        assert_eq!(pp.resource_name, "my-pod");
        assert_eq!(pp.namespace, "default");
        assert_eq!(pp.resource_kind, "pod");
        assert_eq!(pp.ports.len(), 3);
        assert_eq!(pp.cursor, 0);
        assert_eq!(pp.focus, PickerFocus::PortList);
        // Auto-filled from first port
        assert_eq!(pp.local_port_input, "8080");
        assert!(pp.confirmed.is_none());
    }

    #[test]
    fn open_with_empty_ports() {
        let mut pp = PortPicker::new();
        pp.open(
            "svc".to_string(),
            "ns".to_string(),
            "svc".to_string(),
            vec![],
        );
        assert!(pp.visible);
        assert!(pp.ports.is_empty());
        assert!(pp.local_port_input.is_empty());
    }

    #[test]
    fn close_clears_state() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        pp.close();
        assert!(!pp.visible);
        assert!(pp.ports.is_empty());
        assert_eq!(pp.cursor, 0);
        assert!(pp.local_port_input.is_empty());
        assert!(pp.confirmed.is_none());
    }

    // -- Navigation --------------------------------------------------------

    #[test]
    fn move_down_increments_cursor() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        assert_eq!(pp.cursor, 0);
        pp.move_down();
        assert_eq!(pp.cursor, 1);
        // Local port syncs to new selection
        assert_eq!(pp.local_port_input, "9090");
        pp.move_down();
        assert_eq!(pp.cursor, 2);
        assert_eq!(pp.local_port_input, "5432");
    }

    #[test]
    fn move_down_clamps_at_bottom() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            vec![PortInfo {
                container_port: 80,
                protocol: "TCP".to_string(),
                name: "".to_string(),
                container_name: "c".to_string(),
            }],
        );
        pp.move_down();
        assert_eq!(pp.cursor, 0);
    }

    #[test]
    fn move_up_decrements_cursor() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        pp.cursor = 2;
        pp.move_up();
        assert_eq!(pp.cursor, 1);
        assert_eq!(pp.local_port_input, "9090");
    }

    #[test]
    fn move_up_clamps_at_top() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        pp.move_up();
        assert_eq!(pp.cursor, 0);
    }

    #[test]
    fn move_on_empty_does_nothing() {
        let mut pp = PortPicker::new();
        pp.move_up();
        pp.move_down();
        assert_eq!(pp.cursor, 0);
    }

    // -- Focus toggling ----------------------------------------------------

    #[test]
    fn toggle_focus() {
        let mut pp = PortPicker::new();
        assert_eq!(pp.focus, PickerFocus::PortList);
        pp.toggle_focus();
        assert_eq!(pp.focus, PickerFocus::LocalPort);
        pp.toggle_focus();
        assert_eq!(pp.focus, PickerFocus::PortList);
    }

    // -- Local port editing ------------------------------------------------

    #[test]
    fn type_digit_appends() {
        let mut pp = PortPicker::new();
        pp.type_digit('8');
        pp.type_digit('0');
        pp.type_digit('8');
        pp.type_digit('0');
        assert_eq!(pp.local_port_input, "8080");
    }

    #[test]
    fn type_digit_rejects_non_digit() {
        let mut pp = PortPicker::new();
        pp.type_digit('a');
        pp.type_digit('.');
        pp.type_digit('-');
        assert!(pp.local_port_input.is_empty());
    }

    #[test]
    fn type_digit_caps_at_5_chars() {
        let mut pp = PortPicker::new();
        for ch in "123456".chars() {
            pp.type_digit(ch);
        }
        assert_eq!(pp.local_port_input, "12345");
    }

    #[test]
    fn backspace_removes_last() {
        let mut pp = PortPicker::new();
        pp.local_port_input = "808".to_string();
        pp.backspace();
        assert_eq!(pp.local_port_input, "80");
    }

    #[test]
    fn backspace_on_empty_is_noop() {
        let mut pp = PortPicker::new();
        pp.backspace();
        assert!(pp.local_port_input.is_empty());
    }

    #[test]
    fn clear_local_port() {
        let mut pp = PortPicker::new();
        pp.local_port_input = "9090".to_string();
        pp.clear_local_port();
        assert!(pp.local_port_input.is_empty());
    }

    // -- Port selection / confirmation -------------------------------------

    #[test]
    fn selected_container_port_from_cursor() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        assert_eq!(pp.selected_container_port(), 8080);
        pp.cursor = 2;
        assert_eq!(pp.selected_container_port(), 5432);
    }

    #[test]
    fn selected_container_port_empty_returns_zero() {
        let pp = PortPicker::new();
        assert_eq!(pp.selected_container_port(), 0);
    }

    #[test]
    fn effective_local_port_uses_input() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        pp.local_port_input = "3000".to_string();
        assert_eq!(pp.effective_local_port(), 3000);
    }

    #[test]
    fn effective_local_port_falls_back_to_container() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        pp.local_port_input.clear();
        assert_eq!(pp.effective_local_port(), 8080);
    }

    #[test]
    fn parse_local_port_valid() {
        let mut pp = PortPicker::new();
        pp.local_port_input = "9090".to_string();
        assert_eq!(pp.parse_local_port(), Some(9090));
    }

    #[test]
    fn parse_local_port_empty() {
        let pp = PortPicker::new();
        assert_eq!(pp.parse_local_port(), None);
    }

    #[test]
    fn parse_local_port_zero_rejected() {
        let mut pp = PortPicker::new();
        pp.local_port_input = "0".to_string();
        assert_eq!(pp.parse_local_port(), None);
    }

    #[test]
    fn parse_local_port_overflow() {
        let mut pp = PortPicker::new();
        pp.local_port_input = "99999".to_string();
        assert_eq!(pp.parse_local_port(), None);
    }

    #[test]
    fn confirm_with_ports() {
        let mut pp = PortPicker::new();
        pp.open(
            "my-pod".to_string(),
            "default".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        pp.cursor = 1; // 9090 metrics
        pp.local_port_input = "3000".to_string();
        let ok = pp.confirm();
        assert!(ok);
        assert!(!pp.visible);

        let sel = pp.take_selection().expect("should have selection");
        assert_eq!(sel.resource_name, "my-pod");
        assert_eq!(sel.namespace, "default");
        assert_eq!(sel.resource_kind, "pod");
        assert_eq!(sel.container_port, 9090);
        assert_eq!(sel.local_port, 3000);
        assert_eq!(sel.protocol, "TCP");
    }

    #[test]
    fn confirm_with_default_local_port() {
        let mut pp = PortPicker::new();
        pp.open(
            "svc-web".to_string(),
            "prod".to_string(),
            "svc".to_string(),
            sample_ports(),
        );
        // local_port_input is auto-filled to "8080"
        let ok = pp.confirm();
        assert!(ok);
        let sel = pp.take_selection().unwrap();
        assert_eq!(sel.container_port, 8080);
        assert_eq!(sel.local_port, 8080);
    }

    #[test]
    fn confirm_empty_ports_with_manual_input() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            vec![],
        );
        pp.local_port_input = "4000".to_string();
        let ok = pp.confirm();
        assert!(ok);
        let sel = pp.take_selection().unwrap();
        // When no ports discovered, container_port falls back to local_port
        assert_eq!(sel.container_port, 4000);
        assert_eq!(sel.local_port, 4000);
    }

    #[test]
    fn confirm_fails_without_valid_port() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            vec![],
        );
        // No ports, no manual input
        let ok = pp.confirm();
        assert!(!ok);
        assert!(pp.confirmed.is_none());
    }

    #[test]
    fn take_selection_consumes() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        pp.confirm();
        assert!(pp.take_selection().is_some());
        assert!(pp.take_selection().is_none());
    }

    // -- Validation --------------------------------------------------------

    #[test]
    fn is_local_port_valid_with_input() {
        let mut pp = PortPicker::new();
        pp.local_port_input = "8080".to_string();
        assert!(pp.is_local_port_valid());
    }

    #[test]
    fn is_local_port_valid_empty_with_ports() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        pp.local_port_input.clear();
        assert!(pp.is_local_port_valid());
    }

    #[test]
    fn is_local_port_invalid_empty_no_ports() {
        let mut pp = PortPicker::new();
        pp.local_port_input.clear();
        assert!(!pp.is_local_port_valid());
    }

    #[test]
    fn is_local_port_invalid_zero() {
        let mut pp = PortPicker::new();
        pp.local_port_input = "0".to_string();
        assert!(!pp.is_local_port_valid());
    }

    // -- port_count --------------------------------------------------------

    #[test]
    fn port_count_returns_length() {
        let mut pp = PortPicker::new();
        assert_eq!(pp.port_count(), 0);
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        assert_eq!(pp.port_count(), 3);
    }

    // -- Reopen resets state -----------------------------------------------

    #[test]
    fn reopen_resets_cursor_and_selection() {
        let mut pp = PortPicker::new();
        pp.open(
            "p1".to_string(),
            "ns1".to_string(),
            "pod".to_string(),
            sample_ports(),
        );
        pp.move_down();
        pp.move_down();
        pp.confirm();

        pp.open(
            "p2".to_string(),
            "ns2".to_string(),
            "svc".to_string(),
            sample_ports(),
        );
        assert_eq!(pp.cursor, 0);
        assert!(pp.confirmed.is_none());
        assert_eq!(pp.resource_name, "p2");
        assert!(pp.visible);
    }

    // -- selected_protocol -------------------------------------------------

    #[test]
    fn selected_protocol_from_list() {
        let mut pp = PortPicker::new();
        pp.open(
            "pod".to_string(),
            "ns".to_string(),
            "pod".to_string(),
            vec![PortInfo {
                container_port: 53,
                protocol: "UDP".to_string(),
                name: "dns".to_string(),
                container_name: "dns".to_string(),
            }],
        );
        assert_eq!(pp.selected_protocol(), "UDP");
    }

    #[test]
    fn selected_protocol_default_tcp() {
        let pp = PortPicker::new();
        assert_eq!(pp.selected_protocol(), "TCP");
    }

    // -- extract_ports_from_pod --------------------------------------------

    #[test]
    fn extract_pod_ports_basic() {
        let pod_json = serde_json::json!({
            "spec": {
                "containers": [
                    {
                        "name": "web",
                        "ports": [
                            {"containerPort": 8080, "protocol": "TCP", "name": "http"},
                            {"containerPort": 9090, "name": "metrics"}
                        ]
                    },
                    {
                        "name": "sidecar",
                        "ports": [
                            {"containerPort": 15001, "protocol": "TCP"}
                        ]
                    }
                ]
            }
        });
        let ports = extract_ports_from_pod(&pod_json);
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0].container_port, 8080);
        assert_eq!(ports[0].protocol, "TCP");
        assert_eq!(ports[0].name, "http");
        assert_eq!(ports[0].container_name, "web");
        assert_eq!(ports[1].container_port, 9090);
        assert_eq!(ports[1].protocol, "TCP"); // default
        assert_eq!(ports[1].name, "metrics");
        assert_eq!(ports[2].container_port, 15001);
        assert_eq!(ports[2].container_name, "sidecar");
    }

    #[test]
    fn extract_pod_ports_no_ports_field() {
        let pod_json = serde_json::json!({
            "spec": {
                "containers": [{"name": "app"}]
            }
        });
        let ports = extract_ports_from_pod(&pod_json);
        assert!(ports.is_empty());
    }

    #[test]
    fn extract_pod_ports_no_spec() {
        let pod_json = serde_json::json!({});
        let ports = extract_ports_from_pod(&pod_json);
        assert!(ports.is_empty());
    }

    #[test]
    fn extract_pod_ports_skips_zero() {
        let pod_json = serde_json::json!({
            "spec": {
                "containers": [{
                    "name": "app",
                    "ports": [{"containerPort": 0}]
                }]
            }
        });
        let ports = extract_ports_from_pod(&pod_json);
        assert!(ports.is_empty());
    }

    // -- extract_ports_from_service ----------------------------------------

    #[test]
    fn extract_service_ports_basic() {
        let svc_json = serde_json::json!({
            "spec": {
                "ports": [
                    {"port": 80, "targetPort": 8080, "protocol": "TCP", "name": "http"},
                    {"port": 443, "targetPort": 8443, "protocol": "TCP", "name": "https"}
                ]
            }
        });
        let ports = extract_ports_from_service(&svc_json);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].container_port, 80);
        assert_eq!(ports[0].name, "http");
        assert_eq!(ports[0].container_name, "target:8080");
        assert_eq!(ports[1].container_port, 443);
        assert_eq!(ports[1].container_name, "target:8443");
    }

    #[test]
    fn extract_service_ports_no_target() {
        let svc_json = serde_json::json!({
            "spec": {
                "ports": [
                    {"port": 80, "protocol": "TCP"}
                ]
            }
        });
        let ports = extract_ports_from_service(&svc_json);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].container_port, 80);
        // targetPort defaults to same as port
        assert_eq!(ports[0].container_name, "target:80");
    }

    #[test]
    fn extract_service_ports_no_spec() {
        let svc_json = serde_json::json!({});
        let ports = extract_ports_from_service(&svc_json);
        assert!(ports.is_empty());
    }

    #[test]
    fn extract_service_ports_skips_zero() {
        let svc_json = serde_json::json!({
            "spec": {
                "ports": [{"port": 0}]
            }
        });
        let ports = extract_ports_from_service(&svc_json);
        assert!(ports.is_empty());
    }

    // -- PortInfo construction ---------------------------------------------

    #[test]
    fn port_info_fields() {
        let p = PortInfo {
            container_port: 3000,
            protocol: "UDP".to_string(),
            name: "game".to_string(),
            container_name: "server".to_string(),
        };
        assert_eq!(p.container_port, 3000);
        assert_eq!(p.protocol, "UDP");
        assert_eq!(p.name, "game");
        assert_eq!(p.container_name, "server");
    }

    // -- PortForwardSelection construction ---------------------------------

    #[test]
    fn port_forward_selection_fields() {
        let s = PortForwardSelection {
            resource_name: "web-svc".to_string(),
            namespace: "prod".to_string(),
            resource_kind: "svc".to_string(),
            container_port: 80,
            local_port: 3000,
            protocol: "TCP".to_string(),
        };
        assert_eq!(s.resource_name, "web-svc");
        assert_eq!(s.namespace, "prod");
        assert_eq!(s.resource_kind, "svc");
        assert_eq!(s.container_port, 80);
        assert_eq!(s.local_port, 3000);
        assert_eq!(s.protocol, "TCP");
    }

    // -- PickerFocus -------------------------------------------------------

    #[test]
    fn picker_focus_equality() {
        assert_eq!(PickerFocus::PortList, PickerFocus::PortList);
        assert_eq!(PickerFocus::LocalPort, PickerFocus::LocalPort);
        assert_ne!(PickerFocus::PortList, PickerFocus::LocalPort);
    }
}
