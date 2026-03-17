//! HelpOverlay widget — renders context-sensitive keybinding help as a centered
//! modal popup with categorized sections, scrollable when content exceeds the
//! viewport height. Scroll indicators (▲/▼) appear on the right border when
//! content overflows in either direction.
//!
//! # Architecture
//!
//! - [`KeyEntry`] — a single key→description binding
//! - [`KeyCategory`] — a named group of entries (e.g. "Navigation", "Global")
//! - [`ActiveMode`] — the TUI mode that determines which categories to show
//! - [`HelpOverlay`] — stateful overlay: visibility, scroll, mode, rendering
//!
//! Preset category builders (`sidebar_categories()`, `resource_categories()`, etc.)
//! provide the keybinding data for each mode, keeping the overlay decoupled from
//! the rest of the TUI.

use crate::dash::theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single keybinding entry (key label + description).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEntry {
    pub key: &'static str,
    pub desc: &'static str,
}

impl KeyEntry {
    pub const fn new(key: &'static str, desc: &'static str) -> Self {
        Self { key, desc }
    }
}

/// A named category of keybindings (e.g. "Navigation", "Global").
#[derive(Debug, Clone)]
pub struct KeyCategory {
    pub label: &'static str,
    pub entries: Vec<KeyEntry>,
}

impl KeyCategory {
    pub fn new(label: &'static str, entries: Vec<KeyEntry>) -> Self {
        Self { label, entries }
    }

    /// Total rendered lines for this category: header + blank + entries.
    pub fn rendered_line_count(&self) -> usize {
        2 + self.entries.len() // section header + blank line + N entries
    }
}

// ---------------------------------------------------------------------------
// ActiveMode — captures the current TUI mode for context-sensitive help
// ---------------------------------------------------------------------------

/// Represents the current active mode of the TUI, used to generate
/// context-sensitive key binding help in the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveMode {
    /// Sidebar navigation (cluster/namespace tree)
    Sidebar,
    /// Center panel showing a resource table (Pods, Deployments, etc.)
    ResourceTable(String),
    /// Center panel showing Top (node resource usage)
    Top,
    /// Search mode (/ filter active)
    Search,
    /// Command mode (: prompt active)
    CommandMode,
}

impl ActiveMode {
    /// Human-readable label for the help overlay title.
    pub fn label(&self) -> String {
        match self {
            Self::Sidebar => "Sidebar".to_string(),
            Self::ResourceTable(name) => name.clone(),
            Self::Top => "Top".to_string(),
            Self::Search => "Search".to_string(),
            Self::CommandMode => "Command".to_string(),
        }
    }

    /// Return the categorized keybindings for this mode.
    pub fn categories(&self) -> Vec<KeyCategory> {
        match self {
            Self::Search => search_categories(),
            Self::CommandMode => command_mode_categories(),
            Self::Sidebar => sidebar_categories(),
            Self::Top => top_categories(),
            Self::ResourceTable(name) => resource_categories(name),
        }
    }
}

// ---------------------------------------------------------------------------
// Preset keybinding sets
// ---------------------------------------------------------------------------

/// Keybindings for search mode.
pub fn search_categories() -> Vec<KeyCategory> {
    vec![
        KeyCategory::new(
            "Search Mode",
            vec![
                KeyEntry::new("<type>", "Filter by name/namespace"),
                KeyEntry::new("Enter", "Confirm search"),
                KeyEntry::new("ESC", "Cancel search"),
                KeyEntry::new("Backspace", "Delete character"),
            ],
        ),
        global_category(),
    ]
}

/// Keybindings for command mode.
pub fn command_mode_categories() -> Vec<KeyCategory> {
    vec![
        KeyCategory::new(
            "Command Mode",
            vec![
                KeyEntry::new("<type>", "Resource type (e.g. pods, deploy)"),
                KeyEntry::new("Tab", "Accept suggestion"),
                KeyEntry::new("↑/↓", "Navigate suggestions"),
                KeyEntry::new("Enter", "Execute command"),
                KeyEntry::new("ESC", "Cancel"),
                KeyEntry::new("Backspace", "Delete character"),
            ],
        ),
        global_category(),
    ]
}

/// Keybindings for sidebar navigation.
pub fn sidebar_categories() -> Vec<KeyCategory> {
    vec![
        KeyCategory::new(
            "Sidebar Navigation",
            vec![
                KeyEntry::new("j/k", "Move cursor (no selection)"),
                KeyEntry::new("PgUp/Dn", "Jump half page"),
                KeyEntry::new("Home/End", "Jump to first/last"),
                KeyEntry::new("h/l", "Collapse / Expand"),
                KeyEntry::new("Enter", "Select cluster/namespace"),
            ],
        ),
        global_category(),
    ]
}

/// Keybindings for the Top (node resources) view.
pub fn top_categories() -> Vec<KeyCategory> {
    vec![
        KeyCategory::new(
            "Top — Node Resources",
            vec![
                KeyEntry::new("j/k", "Scroll nodes"),
                KeyEntry::new("PgUp/Dn", "Jump half page"),
                KeyEntry::new("Home/End", "Jump to first/last"),
            ],
        ),
        global_category(),
    ]
}

/// Keybindings for a resource table view.
pub fn resource_categories(view_label: &str) -> Vec<KeyCategory> {
    let _ = view_label; // used by caller for title; categories are generic
    vec![
        KeyCategory::new(
            "Navigation",
            vec![
                KeyEntry::new("j/k", "Scroll table rows"),
                KeyEntry::new("PgUp/Dn", "Jump half page"),
                KeyEntry::new("Home/End", "Jump to first/last"),
            ],
        ),
        KeyCategory::new(
            "Resource Actions",
            vec![
                KeyEntry::new("Enter/d", "Describe resource (YAML)"),
                KeyEntry::new("e", "Edit resource (YAML editor)"),
                KeyEntry::new("Ctrl+d", "Delete resource"),
                KeyEntry::new("l", "View logs (pods only)"),
                KeyEntry::new("s", "Shell exec (pods only)"),
                KeyEntry::new("Shift+f", "Port-forward (svc/pod)"),
            ],
        ),
        KeyCategory::new(
            "View Switching",
            vec![
                KeyEntry::new(":", "Command mode (any resource)"),
                KeyEntry::new("/", "Filter by name/namespace"),
                KeyEntry::new("p d s c n e", "Legacy resource shortcuts"),
            ],
        ),
        global_category(),
    ]
}

/// Global keybindings shared across all modes.
pub fn global_category() -> KeyCategory {
    KeyCategory::new(
        "Global",
        vec![
            KeyEntry::new("q/Ctrl+C", "Quit"),
            KeyEntry::new("Tab", "Switch panel (Sidebar ↔ Center)"),
            KeyEntry::new("Shift+Tab", "Switch panel (reverse)"),
            KeyEntry::new("1/2", "Switch tab (Resources/Top)"),
            KeyEntry::new("/", "Search (filter)"),
            KeyEntry::new(":", "Command mode"),
            KeyEntry::new("ESC", "Clear filter / close overlay"),
            KeyEntry::new("r", "Force refresh"),
            KeyEntry::new("?", "Toggle this help"),
        ],
    )
}

// ---------------------------------------------------------------------------
// HelpOverlay — self-contained help popup state and rendering
// ---------------------------------------------------------------------------

/// Modal help overlay that displays context-sensitive key bindings as a
/// centered popup with categorized sections. Scrollable when content exceeds
/// the viewport height.
///
/// ## Rendering layout
///
/// For **wide terminals** (popup ≥ 80 cols), categories are arranged in two
/// columns side-by-side. For narrower terminals, a single-column layout is
/// used with all categories stacked vertically.
///
/// ## Scroll indicators
///
/// When content overflows, ▲ and ▼ indicators appear on the top-right and
/// bottom-right border corners respectively.
pub struct HelpOverlay {
    /// Whether the overlay is currently visible.
    pub visible: bool,
    /// Scroll offset for long help content.
    pub scroll_offset: u16,
    /// Cached viewport inner height (set during render for scroll clamping).
    pub viewport_height: u16,
    /// The active mode when the overlay was opened — determines which bindings to show.
    pub mode: ActiveMode,
}

impl HelpOverlay {
    pub fn new() -> Self {
        Self {
            visible: false,
            scroll_offset: 0,
            viewport_height: 0,
            mode: ActiveMode::Sidebar,
        }
    }

    /// Toggle visibility. When opening, captures the current active mode
    /// and resets scroll position.
    pub fn toggle(&mut self, current_mode: ActiveMode) {
        if self.visible {
            self.close();
        } else {
            self.open(current_mode);
        }
    }

    /// Open the help overlay with the given mode context.
    pub fn open(&mut self, mode: ActiveMode) {
        self.visible = true;
        self.scroll_offset = 0;
        self.mode = mode;
    }

    /// Close the help overlay and reset scroll.
    pub fn close(&mut self) {
        self.visible = false;
        self.scroll_offset = 0;
    }

    /// Scroll up by one line.
    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Scroll down by one line, clamped to max.
    pub fn scroll_down(&mut self) {
        let max = self.max_scroll();
        if self.scroll_offset < max {
            self.scroll_offset += 1;
        }
    }

    /// Page up (half viewport).
    pub fn page_up(&mut self) {
        let jump = (self.viewport_height / 2).max(1);
        self.scroll_offset = self.scroll_offset.saturating_sub(jump);
    }

    /// Page down (half viewport).
    pub fn page_down(&mut self) {
        let max = self.max_scroll();
        let jump = (self.viewport_height / 2).max(1);
        self.scroll_offset = (self.scroll_offset + jump).min(max);
    }

    /// Jump to the beginning.
    pub fn jump_home(&mut self) {
        self.scroll_offset = 0;
    }

    /// Jump to the end.
    pub fn jump_end(&mut self) {
        self.scroll_offset = self.max_scroll();
    }

    // -- Content building ---------------------------------------------------

    /// Build the formatted lines for the current mode's categories.
    /// This is the core rendering data — categories are laid out vertically
    /// with section headers, blank separators, and key→desc entries.
    pub fn build_lines(&self, has_active_filter: bool) -> Vec<Line<'static>> {
        let categories = self.mode.categories();
        let mut lines = Self::render_categories_single_column(&categories);

        // Footer
        lines.push(Line::from(""));
        let footer_text = if has_active_filter {
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

        lines
    }

    /// Render all categories as single-column stacked sections.
    fn render_categories_single_column(categories: &[KeyCategory]) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        for (i, cat) in categories.iter().enumerate() {
            if i > 0 {
                lines.push(Line::from("")); // blank separator between categories
            }
            // Section header
            lines.push(Self::section_header(cat.label));
            lines.push(Line::from(""));

            // Key entries
            for entry in &cat.entries {
                lines.push(Self::key_line(entry));
            }
        }

        lines
    }

    /// Render categories in a two-column layout where left and right columns
    /// each contain a subset of categories. Returns a Vec of Lines where each
    /// line contains spans for both columns side by side.
    fn render_categories_two_column(
        categories: &[KeyCategory],
        col_width: u16,
    ) -> Vec<Line<'static>> {
        if categories.is_empty() {
            return Vec::new();
        }

        // Split categories roughly in half by total line count
        let total_lines: usize = categories.iter().map(|c| c.rendered_line_count()).sum();
        let half = total_lines / 2;
        let mut left_end = 0;
        let mut running = 0usize;
        for (i, cat) in categories.iter().enumerate() {
            running += cat.rendered_line_count();
            left_end = i + 1;
            if running >= half {
                break;
            }
        }
        // Ensure at least one category on each side if possible
        if left_end >= categories.len() && categories.len() > 1 {
            left_end = categories.len() - 1;
        }

        let left_cats = &categories[..left_end];
        let right_cats = &categories[left_end..];

        let left_lines = Self::render_categories_single_column(left_cats);
        let right_lines = Self::render_categories_single_column(right_cats);

        let max_rows = left_lines.len().max(right_lines.len());
        let cw = col_width as usize;
        let mut merged: Vec<Line<'static>> = Vec::with_capacity(max_rows);

        for row in 0..max_rows {
            let mut spans: Vec<Span<'static>> = Vec::new();

            // Left column
            if row < left_lines.len() {
                let left = &left_lines[row];
                let left_text = Self::line_plain_text(left);
                let left_pad = cw.saturating_sub(left_text.len());
                // Copy spans from the left line
                for s in &left.spans {
                    spans.push(s.clone());
                }
                if left_pad > 0 {
                    spans.push(Span::raw(" ".repeat(left_pad)));
                }
            } else {
                spans.push(Span::raw(" ".repeat(cw)));
            }

            // Column separator
            spans.push(Span::styled(
                " │ ".to_string(),
                Style::default().fg(theme::BG3),
            ));

            // Right column
            if row < right_lines.len() {
                let right = &right_lines[row];
                for s in &right.spans {
                    spans.push(s.clone());
                }
            }

            merged.push(Line::from(spans));
        }

        merged
    }

    /// Compute the plain text length of a Line (for column padding).
    fn line_plain_text(line: &Line) -> String {
        let mut s = String::new();
        for span in &line.spans {
            s.push_str(&span.content);
        }
        s
    }

    /// Create a styled section header line.
    fn section_header(label: &str) -> Line<'static> {
        Line::from(Span::styled(
            format!(" {} ", label),
            Style::default()
                .fg(theme::BRIGHT_YELLOW)
                .add_modifier(Modifier::BOLD),
        ))
    }

    /// Create a styled key→description line.
    fn key_line(entry: &KeyEntry) -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!("  {:<12}", entry.key),
                Style::default().fg(theme::BRIGHT_AQUA),
            ),
            Span::styled(entry.desc.to_string(), Style::default().fg(theme::FG)),
        ])
    }

    /// Content line count based on the current mode's categories.
    pub fn content_line_count(&self) -> u16 {
        // Build lines without filter status (doesn't affect count significantly)
        self.build_lines(false).len() as u16
    }

    /// Maximum scroll offset, clamped to prevent over-scrolling.
    fn max_scroll(&self) -> u16 {
        self.content_line_count()
            .saturating_sub(self.viewport_height)
    }

    // -- Rendering ----------------------------------------------------------

    /// Render the help overlay as a centered popup with categorized columns.
    ///
    /// For terminals ≥ 80 columns wide, uses two-column layout.
    /// For narrower terminals, uses single-column layout.
    ///
    /// Scroll indicators (▲/▼) appear when content overflows.
    pub fn render(&mut self, f: &mut Frame, area: Rect, has_active_filter: bool) {
        if !self.visible {
            return;
        }

        let title = format!(" Help — {} ", self.mode.label());
        let categories = self.mode.categories();

        // Decide layout: two-column if terminal is wide enough
        let use_two_col = area.width >= 100 && categories.len() >= 3;
        let popup_width = if use_two_col {
            80u16.min(area.width.saturating_sub(4))
        } else {
            54u16.min(area.width.saturating_sub(4)).max(40.min(area.width))
        };

        // Build content lines
        let mut lines = if use_two_col {
            let col_width = (popup_width.saturating_sub(5)) / 2; // 5 = borders(2) + separator(3)
            Self::render_categories_two_column(&categories, col_width)
        } else {
            Self::render_categories_single_column(&categories)
        };

        // Append footer and attribution
        lines.push(Line::from(""));
        let footer_text = if has_active_filter {
            "  Press ESC to clear filter, ? to close"
        } else {
            "  Press ESC or ? to close"
        };
        lines.push(Line::from(Span::styled(
            footer_text.to_string(),
            Style::default().fg(theme::FG4),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Inspired by k9s (github.com/derailed/k9s)".to_string(),
            Style::default().fg(Color::DarkGray),
        )));

        let content_height = lines.len() as u16;

        // Auto-size popup dimensions
        let max_popup_height = area.height.saturating_sub(2).max(5);
        let popup_height = (content_height + 2).min(max_popup_height); // +2 for borders
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;

        let popup_area = Rect::new(x, y, popup_width, popup_height);
        f.render_widget(Clear, popup_area);

        // Cache viewport height for scroll clamping
        let inner_height = popup_height.saturating_sub(2);
        self.viewport_height = inner_height;

        // Clamp scroll offset to valid range
        let max_scroll = content_height.saturating_sub(inner_height);
        let scroll_offset = self.scroll_offset.min(max_scroll);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::BRIGHT_YELLOW))
            .style(Style::default().bg(theme::BG_HARD));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .scroll((scroll_offset, 0));
        f.render_widget(paragraph, popup_area);

        // Scroll indicators on the right border
        let has_more_above = scroll_offset > 0;
        let has_more_below = scroll_offset < max_scroll && max_scroll > 0;

        if has_more_above {
            let ind = Rect::new(
                popup_area.x + popup_area.width - 1,
                popup_area.y,
                1,
                1,
            );
            f.render_widget(
                Paragraph::new("▲").style(
                    Style::default()
                        .fg(theme::BRIGHT_YELLOW)
                        .bg(theme::BG_HARD),
                ),
                ind,
            );
        }
        if has_more_below {
            let ind = Rect::new(
                popup_area.x + popup_area.width - 1,
                popup_area.y + popup_area.height - 1,
                1,
                1,
            );
            f.render_widget(
                Paragraph::new("▼").style(
                    Style::default()
                        .fg(theme::BRIGHT_YELLOW)
                        .bg(theme::BG_HARD),
                ),
                ind,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Data type tests ----------------------------------------------------

    #[test]
    fn key_entry_construction() {
        let e = KeyEntry::new("j/k", "Move cursor");
        assert_eq!(e.key, "j/k");
        assert_eq!(e.desc, "Move cursor");
    }

    #[test]
    fn key_category_construction() {
        let cat = KeyCategory::new(
            "Navigation",
            vec![
                KeyEntry::new("j", "down"),
                KeyEntry::new("k", "up"),
            ],
        );
        assert_eq!(cat.label, "Navigation");
        assert_eq!(cat.entries.len(), 2);
    }

    #[test]
    fn key_category_rendered_line_count() {
        let cat = KeyCategory::new(
            "Nav",
            vec![
                KeyEntry::new("a", "x"),
                KeyEntry::new("b", "y"),
                KeyEntry::new("c", "z"),
            ],
        );
        // header + blank + 3 entries = 5
        assert_eq!(cat.rendered_line_count(), 5);
    }

    // -- ActiveMode tests ---------------------------------------------------

    #[test]
    fn active_mode_labels() {
        assert_eq!(ActiveMode::Sidebar.label(), "Sidebar");
        assert_eq!(ActiveMode::Top.label(), "Top");
        assert_eq!(ActiveMode::Search.label(), "Search");
        assert_eq!(ActiveMode::CommandMode.label(), "Command");
    }

    #[test]
    fn active_mode_categories_not_empty() {
        let modes = [
            ActiveMode::Search,
            ActiveMode::CommandMode,
            ActiveMode::Sidebar,
            ActiveMode::Top,
            ActiveMode::ResourceTable("Pods".to_string()),
        ];
        for mode in &modes {
            let cats = mode.categories();
            assert!(!cats.is_empty(), "{:?} should have categories", mode);
        }
    }

    // -- Preset category tests ----------------------------------------------

    #[test]
    fn all_presets_end_with_global() {
        let preset_sets: Vec<Vec<KeyCategory>> = vec![
            search_categories(),
            command_mode_categories(),
            sidebar_categories(),
            top_categories(),
            resource_categories("Pods"),
        ];
        for cats in &preset_sets {
            let last = cats.last().expect("should have at least one category");
            assert_eq!(last.label, "Global", "last category should be Global");
            assert!(
                !last.entries.is_empty(),
                "Global category should have entries"
            );
        }
    }

    #[test]
    fn resource_categories_has_actions() {
        let cats = resource_categories("Deployments");
        let action_cat = cats.iter().find(|c| c.label == "Resource Actions");
        assert!(
            action_cat.is_some(),
            "resource view should have Resource Actions category"
        );
        let entries = &action_cat.unwrap().entries;
        // describe, edit, delete, logs, exec, port-forward
        assert!(entries.len() >= 6, "Resource Actions should have ≥6 entries");
    }

    #[test]
    fn global_category_has_essential_keys() {
        let g = global_category();
        let keys: Vec<&str> = g.entries.iter().map(|e| e.key).collect();
        assert!(keys.contains(&"q/Ctrl+C"), "should have quit");
        assert!(keys.contains(&"?"), "should have help toggle");
        assert!(keys.contains(&":"), "should have command mode");
        assert!(keys.contains(&"/"), "should have search");
        assert!(keys.contains(&"r"), "should have refresh");
    }

    // -- HelpOverlay state tests --------------------------------------------

    #[test]
    fn new_overlay_is_hidden() {
        let o = HelpOverlay::new();
        assert!(!o.visible);
        assert_eq!(o.scroll_offset, 0);
        assert_eq!(o.viewport_height, 0);
    }

    #[test]
    fn toggle_opens_and_closes() {
        let mut o = HelpOverlay::new();
        o.toggle(ActiveMode::Sidebar);
        assert!(o.visible);
        assert_eq!(o.mode, ActiveMode::Sidebar);

        o.toggle(ActiveMode::Search); // second toggle closes
        assert!(!o.visible);
    }

    #[test]
    fn open_resets_scroll() {
        let mut o = HelpOverlay::new();
        o.scroll_offset = 10;
        o.open(ActiveMode::Top);
        assert!(o.visible);
        assert_eq!(o.scroll_offset, 0);
        assert_eq!(o.mode, ActiveMode::Top);
    }

    #[test]
    fn close_resets_scroll() {
        let mut o = HelpOverlay::new();
        o.visible = true;
        o.scroll_offset = 5;
        o.close();
        assert!(!o.visible);
        assert_eq!(o.scroll_offset, 0);
    }

    #[test]
    fn scroll_up_saturates_at_zero() {
        let mut o = HelpOverlay::new();
        o.scroll_offset = 0;
        o.scroll_up();
        assert_eq!(o.scroll_offset, 0);
    }

    #[test]
    fn scroll_up_decrements() {
        let mut o = HelpOverlay::new();
        o.scroll_offset = 3;
        o.scroll_up();
        assert_eq!(o.scroll_offset, 2);
    }

    #[test]
    fn scroll_down_clamps_at_max() {
        let mut o = HelpOverlay::new();
        o.open(ActiveMode::Search);
        o.viewport_height = 100; // larger than content
        o.scroll_down();
        assert_eq!(o.scroll_offset, 0); // can't scroll past content
    }

    #[test]
    fn page_up_half_viewport() {
        let mut o = HelpOverlay::new();
        o.viewport_height = 10;
        o.scroll_offset = 8;
        o.page_up();
        assert_eq!(o.scroll_offset, 3); // 8 - 5 = 3
    }

    #[test]
    fn page_down_half_viewport() {
        let mut o = HelpOverlay::new();
        o.open(ActiveMode::Sidebar);
        o.viewport_height = 10;
        o.page_down();
        // Should jump by 5 (viewport/2), clamped to max_scroll
        assert!(o.scroll_offset <= o.content_line_count());
    }

    #[test]
    fn jump_home_resets_to_zero() {
        let mut o = HelpOverlay::new();
        o.scroll_offset = 42;
        o.jump_home();
        assert_eq!(o.scroll_offset, 0);
    }

    #[test]
    fn jump_end_goes_to_max() {
        let mut o = HelpOverlay::new();
        o.open(ActiveMode::Search);
        o.viewport_height = 5;
        o.jump_end();
        let max = o.content_line_count().saturating_sub(5);
        assert_eq!(o.scroll_offset, max);
    }

    // -- Content line building tests ----------------------------------------

    #[test]
    fn build_lines_not_empty() {
        let o = HelpOverlay {
            visible: true,
            scroll_offset: 0,
            viewport_height: 0,
            mode: ActiveMode::Sidebar,
        };
        let lines = o.build_lines(false);
        assert!(!lines.is_empty());
    }

    #[test]
    fn build_lines_with_filter_changes_footer() {
        let o = HelpOverlay {
            visible: true,
            scroll_offset: 0,
            viewport_height: 0,
            mode: ActiveMode::Sidebar,
        };
        let no_filter = o.build_lines(false);
        let with_filter = o.build_lines(true);

        // Both should have content, but differ in footer text
        assert!(!no_filter.is_empty());
        assert!(!with_filter.is_empty());
        // The with_filter version should mention "clear filter"
        let filter_text: String = with_filter
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("");
        assert!(filter_text.contains("clear filter"));
    }

    #[test]
    fn content_line_count_positive() {
        for mode in [
            ActiveMode::Search,
            ActiveMode::CommandMode,
            ActiveMode::Sidebar,
            ActiveMode::Top,
            ActiveMode::ResourceTable("Pods".to_string()),
        ] {
            let o = HelpOverlay {
                visible: true,
                scroll_offset: 0,
                viewport_height: 0,
                mode,
            };
            assert!(
                o.content_line_count() > 0,
                "content_line_count should be > 0 for {:?}",
                o.mode,
            );
        }
    }

    #[test]
    fn two_column_layout_produces_lines() {
        let categories = resource_categories("Pods");
        let lines = HelpOverlay::render_categories_two_column(&categories, 35);
        assert!(!lines.is_empty(), "two-column layout should produce lines");
    }

    #[test]
    fn two_column_layout_row_count_leq_single() {
        let categories = resource_categories("Pods");
        let single = HelpOverlay::render_categories_single_column(&categories);
        let dual = HelpOverlay::render_categories_two_column(&categories, 35);
        // Two-column should have fewer or equal rows (content is side-by-side)
        assert!(
            dual.len() <= single.len(),
            "two-column ({}) should have ≤ rows than single-column ({})",
            dual.len(),
            single.len(),
        );
    }

    #[test]
    fn single_column_includes_all_entries() {
        let categories = sidebar_categories();
        let total_entries: usize = categories.iter().map(|c| c.entries.len()).sum();
        let lines = HelpOverlay::render_categories_single_column(&categories);

        // Count lines that have the key style (BRIGHT_AQUA) — these are entry lines
        let entry_lines = lines
            .iter()
            .filter(|l| {
                l.spans
                    .first()
                    .is_some_and(|s| s.style.fg == Some(theme::BRIGHT_AQUA))
            })
            .count();
        assert_eq!(
            entry_lines, total_entries,
            "single column should render all {} entries",
            total_entries,
        );
    }

    // -- Line helper tests --------------------------------------------------

    #[test]
    fn section_header_styled_correctly() {
        let line = HelpOverlay::section_header("Test Section");
        assert_eq!(line.spans.len(), 1);
        let span = &line.spans[0];
        assert!(span.content.contains("Test Section"));
        assert_eq!(span.style.fg, Some(theme::BRIGHT_YELLOW));
    }

    #[test]
    fn key_line_styled_correctly() {
        let entry = KeyEntry::new("j/k", "Move cursor");
        let line = HelpOverlay::key_line(&entry);
        assert_eq!(line.spans.len(), 2);
        // Key span: BRIGHT_AQUA
        assert_eq!(line.spans[0].style.fg, Some(theme::BRIGHT_AQUA));
        assert!(line.spans[0].content.contains("j/k"));
        // Desc span: FG
        assert_eq!(line.spans[1].style.fg, Some(theme::FG));
        assert_eq!(line.spans[1].content, "Move cursor");
    }

    #[test]
    fn line_plain_text_concatenates() {
        let line = Line::from(vec![
            Span::raw("hello"),
            Span::raw(" "),
            Span::raw("world"),
        ]);
        assert_eq!(HelpOverlay::line_plain_text(&line), "hello world");
    }
}
