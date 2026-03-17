//! Toast notification system for the TUI dashboard.
//!
//! Toasts appear at the bottom-right of the terminal and auto-dismiss after a
//! configurable duration (~5 seconds by default). They are non-blocking and
//! do not interrupt user interaction.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::dash::theme;

// ---------------------------------------------------------------------------
// Toast level
// ---------------------------------------------------------------------------

/// Severity level of a toast notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    /// Informational message (green border)
    Info,
    /// Warning message (yellow border)
    Warn,
    /// Error message (red border)
    Error,
    /// Success message (bright green border)
    Success,
}

impl ToastLevel {
    /// Border / accent color for the toast level.
    pub fn color(&self) -> Color {
        match self {
            Self::Info => theme::BRIGHT_BLUE,
            Self::Warn => theme::BRIGHT_YELLOW,
            Self::Error => theme::BRIGHT_RED,
            Self::Success => theme::BRIGHT_GREEN,
        }
    }

    /// Short prefix icon for the toast message.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Info => "ℹ ",
            Self::Warn => "⚠ ",
            Self::Error => "✖ ",
            Self::Success => "✔ ",
        }
    }
}

// ---------------------------------------------------------------------------
// Toast item
// ---------------------------------------------------------------------------

/// A single toast notification.
#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub level: ToastLevel,
    pub created_at: Instant,
    pub ttl: Duration,
}

impl Toast {
    pub fn new(message: impl Into<String>, level: ToastLevel) -> Self {
        Self {
            message: message.into(),
            level,
            created_at: Instant::now(),
            ttl: Duration::from_secs(5),
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Returns true if this toast has expired.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.ttl
    }

    /// Returns remaining fraction (1.0 = just created, 0.0 = expired).
    /// Used for fade-out effect on the last second.
    pub fn remaining_fraction(&self) -> f64 {
        let elapsed = self.created_at.elapsed().as_secs_f64();
        let total = self.ttl.as_secs_f64();
        if total <= 0.0 {
            return 0.0;
        }
        (1.0 - elapsed / total).max(0.0)
    }
}

// ---------------------------------------------------------------------------
// Toast manager
// ---------------------------------------------------------------------------

/// Maximum number of toasts displayed simultaneously.
const MAX_VISIBLE_TOASTS: usize = 5;

/// Manages a queue of toast notifications.
#[derive(Debug, Clone)]
pub struct ToastManager {
    toasts: VecDeque<Toast>,
}

impl Default for ToastManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ToastManager {
    pub fn new() -> Self {
        Self {
            toasts: VecDeque::new(),
        }
    }

    /// Push a new toast notification.
    pub fn push(&mut self, toast: Toast) {
        // Deduplicate: if the same message+level is already queued, reset its timer
        for existing in self.toasts.iter_mut() {
            if existing.message == toast.message && existing.level == toast.level {
                existing.created_at = Instant::now();
                existing.ttl = toast.ttl;
                return;
            }
        }
        self.toasts.push_back(toast);
        // Cap the queue size
        while self.toasts.len() > MAX_VISIBLE_TOASTS * 2 {
            self.toasts.pop_front();
        }
    }

    /// Convenience: push an error toast.
    pub fn error(&mut self, message: impl Into<String>) {
        self.push(Toast::new(message, ToastLevel::Error));
    }

    /// Convenience: push a warning toast.
    pub fn warn(&mut self, message: impl Into<String>) {
        self.push(Toast::new(message, ToastLevel::Warn));
    }

    /// Convenience: push an info toast.
    pub fn info(&mut self, message: impl Into<String>) {
        self.push(Toast::new(message, ToastLevel::Info));
    }

    /// Convenience: push a success toast.
    pub fn success(&mut self, message: impl Into<String>) {
        self.push(Toast::new(message, ToastLevel::Success));
    }

    /// Remove expired toasts. Returns true if any were removed (needs redraw).
    pub fn gc(&mut self) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|t| !t.is_expired());
        self.toasts.len() != before
    }

    /// Returns true if there are any active toasts.
    pub fn has_toasts(&self) -> bool {
        !self.toasts.is_empty()
    }

    /// Returns the visible toasts (most recent first, up to MAX_VISIBLE_TOASTS).
    pub fn visible(&self) -> impl Iterator<Item = &Toast> {
        self.toasts.iter().rev().take(MAX_VISIBLE_TOASTS)
    }

    /// Number of active (non-expired) toasts.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.toasts.len()
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render toast notifications as an overlay at the bottom-right of the screen.
///
/// Toasts stack upward from the bottom, each occupying 3 rows (1 border top,
/// 1 content, 1 border bottom). They are rendered after the main UI so they
/// overlay other content.
pub fn render_toasts(f: &mut Frame, toasts: &ToastManager, area: Rect) {
    let visible: Vec<&Toast> = toasts.visible().collect();
    if visible.is_empty() {
        return;
    }

    // Toast dimensions
    let toast_width = area.width.clamp(20, 60);
    let toast_height: u16 = 3; // border + 1 line content + border
    let right_margin: u16 = 1;
    let bottom_margin: u16 = 1; // above the status bar area

    // Stack toasts upward from bottom-right
    for (i, toast) in visible.iter().enumerate() {
        let y_offset = bottom_margin + (i as u16) * toast_height;
        let toast_y = area.height.saturating_sub(y_offset + toast_height);

        // Don't render if it would go off the top of the screen
        if toast_y < 1 {
            break;
        }

        let toast_x = area.width.saturating_sub(toast_width + right_margin);

        let toast_area = Rect::new(toast_x, toast_y, toast_width, toast_height);

        // Determine style based on remaining time (fade effect in last second)
        let remaining = toast.remaining_fraction();
        let border_color = toast.level.color();
        let fg_color = if remaining < 0.2 {
            theme::FG4 // Faded
        } else {
            theme::FG
        };

        // Build the content line with icon + message (truncated to fit)
        let icon = toast.level.icon();
        let max_msg_width = (toast_width as usize).saturating_sub(4 + icon.len()); // 2 borders + 2 padding
        let msg = if toast.message.len() > max_msg_width {
            format!("{}…", &toast.message[..max_msg_width.saturating_sub(1)])
        } else {
            toast.message.clone()
        };

        let content = Line::from(vec![
            Span::styled(
                icon,
                Style::default()
                    .fg(border_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(msg, Style::default().fg(fg_color)),
        ]);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(theme::BG_HARD));

        let paragraph = Paragraph::new(content).block(block);

        // Clear the area first so toast overlays cleanly
        f.render_widget(Clear, toast_area);
        f.render_widget(paragraph, toast_area);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_auto_expires() {
        let toast = Toast::new("test", ToastLevel::Error).with_ttl(Duration::from_millis(0));
        // With zero TTL, should be immediately expired
        assert!(toast.is_expired());
    }

    #[test]
    fn toast_not_expired_within_ttl() {
        let toast = Toast::new("test", ToastLevel::Info).with_ttl(Duration::from_secs(10));
        assert!(!toast.is_expired());
    }

    #[test]
    fn toast_manager_push_and_gc() {
        let mut mgr = ToastManager::new();
        assert!(!mgr.has_toasts());

        mgr.error("Something failed");
        assert!(mgr.has_toasts());
        assert_eq!(mgr.len(), 1);

        // GC should not remove non-expired toast
        let removed = mgr.gc();
        assert!(!removed);
        assert_eq!(mgr.len(), 1);
    }

    #[test]
    fn toast_manager_deduplicates() {
        let mut mgr = ToastManager::new();
        mgr.error("Connection failed");
        mgr.error("Connection failed");
        assert_eq!(mgr.len(), 1); // Deduplicated
    }

    #[test]
    fn toast_manager_different_levels_not_deduped() {
        let mut mgr = ToastManager::new();
        mgr.error("msg");
        mgr.warn("msg");
        assert_eq!(mgr.len(), 2); // Different levels = different toasts
    }

    #[test]
    fn toast_remaining_fraction() {
        let toast = Toast::new("test", ToastLevel::Info).with_ttl(Duration::from_secs(5));
        let frac = toast.remaining_fraction();
        // Just created, should be close to 1.0
        assert!(frac > 0.9);
    }

    #[test]
    fn toast_gc_removes_expired() {
        let mut mgr = ToastManager::new();
        mgr.push(Toast::new("old", ToastLevel::Info).with_ttl(Duration::from_millis(0)));
        mgr.push(Toast::new("new", ToastLevel::Info).with_ttl(Duration::from_secs(60)));

        let removed = mgr.gc();
        assert!(removed);
        assert_eq!(mgr.len(), 1);
        assert_eq!(mgr.visible().next().unwrap().message, "new");
    }

    #[test]
    fn toast_level_colors() {
        // Just ensure they don't panic
        let _ = ToastLevel::Info.color();
        let _ = ToastLevel::Warn.color();
        let _ = ToastLevel::Error.color();
        let _ = ToastLevel::Success.color();
    }

    #[test]
    fn toast_level_icons() {
        assert!(!ToastLevel::Info.icon().is_empty());
        assert!(!ToastLevel::Error.icon().is_empty());
    }
}
