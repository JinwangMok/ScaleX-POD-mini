//! YAML describe modal overlay — displays a scrollable read-only view of a
//! Kubernetes resource's describe output (similar to `kubectl describe`).

use crate::dash::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// State for the YAML/describe modal overlay.
#[derive(Debug, Clone)]
pub struct YamlModal {
    pub visible: bool,
    pub content: String,
    pub title: String,
    pub scroll_offset: u16,
    pub viewport_height: u16,
    pub line_count: u16,
    pub resource_name: String,
    pub resource_kind: String,
}

impl YamlModal {
    pub fn new() -> Self {
        Self {
            visible: false,
            content: String::new(),
            title: String::new(),
            scroll_offset: 0,
            viewport_height: 0,
            line_count: 0,
            resource_name: String::new(),
            resource_kind: String::new(),
        }
    }

    pub fn open(&mut self, kind: &str, name: &str, content: String) {
        self.resource_kind = kind.to_string();
        self.resource_name = name.to_string();
        self.title = format!(" Describe: {} ({}) ", name, kind);
        self.line_count = content.lines().count() as u16;
        self.content = content;
        self.scroll_offset = 0;
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.content.clear();
        self.scroll_offset = 0;
        self.line_count = 0;
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        let max = self.max_scroll();
        if self.scroll_offset < max {
            self.scroll_offset += 1;
        }
    }

    pub fn page_up(&mut self) {
        let jump = (self.viewport_height / 2).max(1);
        self.scroll_offset = self.scroll_offset.saturating_sub(jump);
    }

    pub fn page_down(&mut self) {
        let jump = (self.viewport_height / 2).max(1);
        let max = self.max_scroll();
        self.scroll_offset = (self.scroll_offset + jump).min(max);
    }

    pub fn jump_home(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn jump_end(&mut self) {
        self.scroll_offset = self.max_scroll();
    }

    fn max_scroll(&self) -> u16 {
        self.line_count.saturating_sub(self.viewport_height)
    }

    /// Render the modal overlay as a centered popup.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        if !self.visible {
            return;
        }

        // Popup dimensions: 80% width, 80% height
        let popup_width = (area.width * 80 / 100).max(40).min(area.width.saturating_sub(4));
        let popup_height = (area.height * 80 / 100).max(10).min(area.height.saturating_sub(2));
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::BRIGHT_AQUA))
            .style(Style::default().bg(theme::BG_HARD));

        let inner = block.inner(popup_area);
        self.viewport_height = inner.height;

        // Clamp scroll
        let max = self.max_scroll();
        if self.scroll_offset > max {
            self.scroll_offset = max;
        }

        let lines: Vec<Line<'_>> = self.content
            .lines()
            .map(|line| {
                let trimmed = line.trim();
                if !trimmed.is_empty()
                    && trimmed.ends_with(':')
                    && !trimmed.contains("  ")
                    && trimmed.chars().next().map_or(false, |c| c.is_alphabetic())
                {
                    Line::from(Span::styled(
                        line,
                        Style::default()
                            .fg(theme::BRIGHT_YELLOW)
                            .add_modifier(Modifier::BOLD),
                    ))
                } else if let Some(colon_pos) = trimmed.find(':') {
                    let key_part = &trimmed[..colon_pos];
                    if !key_part.is_empty()
                        && !key_part.contains(' ')
                        && colon_pos < trimmed.len() - 1
                    {
                        let indent = line.len() - line.trim_start().len();
                        let indent_str = &line[..indent];
                        let value_part = &trimmed[colon_pos + 1..];
                        Line::from(vec![
                            Span::raw(indent_str),
                            Span::styled(key_part, Style::default().fg(theme::BRIGHT_AQUA)),
                            Span::styled(":", Style::default().fg(theme::FG4)),
                            Span::styled(value_part, Style::default().fg(theme::FG)),
                        ])
                    } else {
                        Line::from(Span::styled(line, Style::default().fg(theme::FG)))
                    }
                } else {
                    Line::from(Span::styled(line, Style::default().fg(theme::FG)))
                }
            })
            .collect();

        let paragraph = Paragraph::new(lines)
            .block(block)
            .style(Style::default().bg(theme::BG_HARD))
            .scroll((self.scroll_offset, 0));

        f.render_widget(paragraph, popup_area);
    }
}

impl Default for YamlModal {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_modal_is_hidden() {
        let m = YamlModal::new();
        assert!(!m.visible);
        assert!(m.content.is_empty());
    }

    #[test]
    fn open_sets_visible_and_content() {
        let mut m = YamlModal::new();
        m.open("Pod", "nginx", "Name: nginx\nNamespace: default\n".to_string());
        assert!(m.visible);
        assert_eq!(m.resource_name, "nginx");
        assert_eq!(m.resource_kind, "Pod");
        assert_eq!(m.scroll_offset, 0);
        assert_eq!(m.line_count, 2);
    }

    #[test]
    fn close_clears_state() {
        let mut m = YamlModal::new();
        m.open("Pod", "nginx", "content\nline2\n".to_string());
        m.close();
        assert!(!m.visible);
        assert!(m.content.is_empty());
        assert_eq!(m.scroll_offset, 0);
        assert_eq!(m.line_count, 0);
    }

    #[test]
    fn scroll_down_clamps() {
        let mut m = YamlModal::new();
        m.open("Pod", "test", "a\nb\nc\nd\ne\n".to_string());
        m.viewport_height = 3;
        m.scroll_down();
        assert_eq!(m.scroll_offset, 1);
        m.scroll_down();
        assert_eq!(m.scroll_offset, 2);
        m.scroll_down();
        assert_eq!(m.scroll_offset, 2); // clamped
    }

    #[test]
    fn default_is_same_as_new() {
        let d = YamlModal::default();
        assert!(!d.visible);
        assert!(d.content.is_empty());
    }
}
