//! Log viewer modal — streams and displays pod container logs in a scrollable overlay.
//!
//! In k9s, pressing `l` on a pod opens a log viewer. If the pod has multiple containers,
//! a container selector is shown first. The log viewer streams logs via the kube-rs API
//! and displays them in a scrollable text area.
//!
//! # Key bindings (when log viewer is visible)
//!
//! | Key        | Action                     |
//! |------------|----------------------------|
//! | `j`/`↓`    | Scroll down one line       |
//! | `k`/`↑`    | Scroll up one line         |
//! | `G`/`End`  | Jump to bottom (tail)      |
//! | `g`/`Home` | Jump to top                |
//! | `ESC`/`q`  | Close log viewer           |
//! | `f`        | Toggle auto-follow (tail)  |

use crate::dash::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::buffer::Buffer;
use ratatui::Frame;

// ---------------------------------------------------------------------------
// LogViewer state
// ---------------------------------------------------------------------------

/// State for the log viewer modal overlay.
///
/// When `visible` is true, this modal intercepts keyboard events and renders
/// a full-screen log display over the center panel.
#[derive(Debug, Clone)]
pub struct LogViewer {
    /// Whether the log viewer is currently shown.
    pub visible: bool,
    /// Pod name being viewed.
    pub pod_name: String,
    /// Pod namespace.
    pub namespace: String,
    /// Container name being viewed.
    pub container_name: String,
    /// Log lines accumulated from the stream.
    pub lines: Vec<String>,
    /// Current scroll offset (0 = top).
    pub scroll_offset: usize,
    /// Whether auto-follow (tail) mode is active.
    pub auto_follow: bool,
    /// Cluster name for this log stream (for display in title).
    pub cluster_name: String,
}

impl LogViewer {
    /// Create a new hidden log viewer.
    pub fn new() -> Self {
        Self {
            visible: false,
            pod_name: String::new(),
            namespace: String::new(),
            container_name: String::new(),
            lines: Vec::new(),
            scroll_offset: 0,
            auto_follow: true,
            cluster_name: String::new(),
        }
    }

    /// Open the log viewer for a specific pod/container.
    pub fn open(
        &mut self,
        pod_name: &str,
        namespace: &str,
        container_name: &str,
        cluster_name: &str,
    ) {
        self.pod_name = pod_name.to_string();
        self.namespace = namespace.to_string();
        self.container_name = container_name.to_string();
        self.cluster_name = cluster_name.to_string();
        self.lines.clear();
        self.scroll_offset = 0;
        self.auto_follow = true;
        self.visible = true;
    }

    /// Close the log viewer.
    pub fn close(&mut self) {
        self.visible = false;
        self.lines.clear();
        self.scroll_offset = 0;
    }

    /// Append a log line. If auto_follow, scroll to bottom.
    pub fn push_line(&mut self, line: String) {
        self.lines.push(line);
        if self.auto_follow {
            // Will be clamped in render
            self.scroll_offset = self.lines.len().saturating_sub(1);
        }
    }

    /// Scroll up by one line.
    pub fn scroll_up(&mut self) {
        self.auto_follow = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Scroll down by one line.
    pub fn scroll_down(&mut self) {
        if self.scroll_offset < self.lines.len().saturating_sub(1) {
            self.scroll_offset += 1;
        }
    }

    /// Jump to the top of the log.
    pub fn jump_to_top(&mut self) {
        self.auto_follow = false;
        self.scroll_offset = 0;
    }

    /// Jump to the bottom of the log and enable auto-follow.
    pub fn jump_to_bottom(&mut self) {
        self.auto_follow = true;
        self.scroll_offset = self.lines.len().saturating_sub(1);
    }

    /// Toggle auto-follow mode.
    pub fn toggle_follow(&mut self) {
        self.auto_follow = !self.auto_follow;
        if self.auto_follow {
            self.scroll_offset = self.lines.len().saturating_sub(1);
        }
    }

    /// Page up (half viewport).
    pub fn page_up(&mut self, viewport_height: usize) {
        self.auto_follow = false;
        let jump = (viewport_height / 2).max(1);
        self.scroll_offset = self.scroll_offset.saturating_sub(jump);
    }

    /// Page down (half viewport).
    pub fn page_down(&mut self, viewport_height: usize) {
        let jump = (viewport_height / 2).max(1);
        self.scroll_offset = (self.scroll_offset + jump).min(self.lines.len().saturating_sub(1));
    }

    // -- Rendering ---------------------------------------------------------

    /// Render the log viewer modal overlay (full-screen).
    pub fn render(&self, f: &mut Frame, area: Rect) {
        if !self.visible {
            return;
        }

        // Use nearly full screen
        let margin = 2u16;
        let popup_width = area.width.saturating_sub(margin * 2).max(40);
        let popup_height = area.height.saturating_sub(margin * 2).max(10);
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        // Dim background
        f.render_widget(DimOverlay, area);

        // Clear popup area
        f.render_widget(Clear, popup_area);

        // Title
        let follow_indicator = if self.auto_follow { " [FOLLOW]" } else { "" };
        let title = format!(
            " Logs: {}/{} ({}) {} ",
            self.namespace, self.pod_name, self.container_name, follow_indicator
        );

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::BRIGHT_AQUA))
            .style(Style::default().bg(theme::BG));
        f.render_widget(block, popup_area);

        // Inner content area
        let inner = Rect::new(
            popup_area.x + 1,
            popup_area.y + 1,
            popup_area.width.saturating_sub(2),
            popup_area.height.saturating_sub(3), // -2 for border, -1 for footer
        );

        let viewport_height = inner.height as usize;

        // Compute visible range
        let total = self.lines.len();
        let start = self.scroll_offset;
        let end = (start + viewport_height).min(total);

        let mut display_lines: Vec<Line<'_>> = Vec::with_capacity(viewport_height);

        if total == 0 {
            display_lines.push(Line::from(Span::styled(
                "  Waiting for logs...",
                Style::default().fg(theme::FG4),
            )));
        } else {
            for i in start..end {
                let line_text = &self.lines[i];
                // Color log lines based on content
                let style = if line_text.contains("ERROR") || line_text.contains("error") || line_text.contains("FATAL") {
                    Style::default().fg(theme::BRIGHT_RED)
                } else if line_text.contains("WARN") || line_text.contains("warn") {
                    Style::default().fg(theme::BRIGHT_YELLOW)
                } else {
                    Style::default().fg(theme::FG)
                };
                display_lines.push(Line::from(Span::styled(line_text.as_str(), style)));
            }
        }

        let paragraph = Paragraph::new(display_lines).style(Style::default().bg(theme::BG));
        f.render_widget(paragraph, inner);

        // Footer with key hints and scroll position
        let footer_area = Rect::new(
            popup_area.x + 1,
            popup_area.y + popup_area.height.saturating_sub(2),
            popup_area.width.saturating_sub(2),
            1,
        );

        let scroll_info = if total > 0 {
            format!(" {}/{} ", end, total)
        } else {
            " 0/0 ".to_string()
        };

        let footer = Line::from(vec![
            Span::styled(
                " ↑↓/jk: scroll  g/G: top/bottom  f: follow  ESC: close",
                Style::default().fg(theme::FG4),
            ),
            Span::styled(
                scroll_info,
                Style::default()
                    .fg(theme::BRIGHT_AQUA)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let footer_para = Paragraph::new(footer).style(Style::default().bg(theme::BG));
        f.render_widget(footer_para, footer_area);
    }
}

impl Default for LogViewer {
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_viewer_is_hidden() {
        let viewer = LogViewer::new();
        assert!(!viewer.visible);
        assert!(viewer.lines.is_empty());
        assert_eq!(viewer.scroll_offset, 0);
        assert!(viewer.auto_follow);
    }

    #[test]
    fn default_is_same_as_new() {
        let viewer = LogViewer::default();
        assert!(!viewer.visible);
    }

    #[test]
    fn open_sets_state() {
        let mut viewer = LogViewer::new();
        viewer.open("my-pod", "default", "nginx", "tower");
        assert!(viewer.visible);
        assert_eq!(viewer.pod_name, "my-pod");
        assert_eq!(viewer.namespace, "default");
        assert_eq!(viewer.container_name, "nginx");
        assert_eq!(viewer.cluster_name, "tower");
        assert!(viewer.auto_follow);
    }

    #[test]
    fn close_clears_state() {
        let mut viewer = LogViewer::new();
        viewer.open("pod", "ns", "container", "cluster");
        viewer.push_line("test".to_string());
        viewer.close();
        assert!(!viewer.visible);
        assert!(viewer.lines.is_empty());
        assert_eq!(viewer.scroll_offset, 0);
    }

    #[test]
    fn push_line_auto_follows() {
        let mut viewer = LogViewer::new();
        viewer.open("pod", "ns", "c", "cl");
        viewer.push_line("line 1".to_string());
        assert_eq!(viewer.scroll_offset, 0);
        viewer.push_line("line 2".to_string());
        assert_eq!(viewer.scroll_offset, 1);
        viewer.push_line("line 3".to_string());
        assert_eq!(viewer.scroll_offset, 2);
    }

    #[test]
    fn scroll_up_disables_follow() {
        let mut viewer = LogViewer::new();
        viewer.open("pod", "ns", "c", "cl");
        for i in 0..10 {
            viewer.push_line(format!("line {}", i));
        }
        assert!(viewer.auto_follow);
        viewer.scroll_up();
        assert!(!viewer.auto_follow);
        assert_eq!(viewer.scroll_offset, 8); // was at 9, scrolled up 1
    }

    #[test]
    fn scroll_down_clamps() {
        let mut viewer = LogViewer::new();
        viewer.open("pod", "ns", "c", "cl");
        viewer.push_line("only line".to_string());
        viewer.scroll_down();
        assert_eq!(viewer.scroll_offset, 0); // can't go past end
    }

    #[test]
    fn jump_to_top() {
        let mut viewer = LogViewer::new();
        viewer.open("pod", "ns", "c", "cl");
        for i in 0..10 {
            viewer.push_line(format!("line {}", i));
        }
        viewer.jump_to_top();
        assert_eq!(viewer.scroll_offset, 0);
        assert!(!viewer.auto_follow);
    }

    #[test]
    fn jump_to_bottom_enables_follow() {
        let mut viewer = LogViewer::new();
        viewer.open("pod", "ns", "c", "cl");
        for i in 0..10 {
            viewer.push_line(format!("line {}", i));
        }
        viewer.auto_follow = false;
        viewer.scroll_offset = 0;
        viewer.jump_to_bottom();
        assert_eq!(viewer.scroll_offset, 9);
        assert!(viewer.auto_follow);
    }

    #[test]
    fn toggle_follow() {
        let mut viewer = LogViewer::new();
        viewer.open("pod", "ns", "c", "cl");
        assert!(viewer.auto_follow);
        viewer.toggle_follow();
        assert!(!viewer.auto_follow);
        viewer.toggle_follow();
        assert!(viewer.auto_follow);
    }

    #[test]
    fn page_up_and_down() {
        let mut viewer = LogViewer::new();
        viewer.open("pod", "ns", "c", "cl");
        for i in 0..50 {
            viewer.push_line(format!("line {}", i));
        }
        // At bottom (49)
        viewer.page_up(20);
        assert_eq!(viewer.scroll_offset, 39); // 49 - 10 (half of 20)
        assert!(!viewer.auto_follow);

        viewer.page_down(20);
        assert_eq!(viewer.scroll_offset, 49); // 39 + 10
    }
}
