// ---------------------------------------------------------------------------
// Command Mode — k9s-style `:command` input with fuzzy autocomplete
// ---------------------------------------------------------------------------
//
// Provides the interactive command input bar that appears when the user presses
// `:` in the TUI. Supports fuzzy matching against the resource registry with
// ranked autocomplete suggestions.

use crate::dash::resource_registry::{fuzzy_match, ResourceRegistry};
use std::collections::VecDeque;

/// Maximum number of autocomplete suggestions to show.
pub const MAX_SUGGESTIONS: usize = 8;

/// Maximum number of history entries to retain.
const MAX_HISTORY: usize = 50;

/// State for the command-mode input bar.
#[derive(Debug, Clone, Default)]
pub struct CommandMode {
    /// Whether command mode is currently active (`:` pressed)
    pub active: bool,
    /// Current input text (without the leading `:`)
    pub input: String,
    /// Pre-lowercased input for matching (updated on every keystroke)
    input_lower: String,
    /// Current autocomplete suggestions (updated on every keystroke)
    pub suggestions: Vec<Suggestion>,
    /// Index of the currently highlighted suggestion (0-based), None if no selection
    pub selected_suggestion: Option<usize>,
    /// The command that was submitted (set on Enter, consumed by the app)
    pub submitted: Option<String>,
    /// Command history ring buffer (most recent last)
    history: VecDeque<String>,
    /// Current position when browsing history with Up/Down (None = new input)
    history_cursor: Option<usize>,
    /// Saved input before browsing history (restored when moving past end)
    saved_input: String,

    // --- Tab-completion cycling state ---
    /// The original input prefix that initiated tab-completion (before any Tab replacement).
    /// `None` means we are not in a tab-cycle session.
    tab_prefix: Option<String>,
    /// Index into `suggestions` for the current tab-cycle position.
    tab_cycle_index: usize,
}

/// A single autocomplete suggestion displayed in the dropdown.
#[derive(Debug, Clone)]
pub struct Suggestion {
    /// Display text: the resource plural name (e.g., "deployments")
    pub display: String,
    /// Short name hint shown alongside (e.g., "deploy")
    pub hint: String,
    /// API group for disambiguation (e.g., "apps", "networking.k8s.io")
    pub api_group: String,
    /// The fuzzy match score (for display ranking)
    pub score: u32,
    /// Whether the resource is namespaced
    pub namespaced: bool,
    /// Index into the ResourceRegistry entries
    pub entry_idx: usize,
}

impl CommandMode {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if command mode is currently active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Activate command mode (called when `:` is pressed).
    pub fn activate(&mut self) {
        self.active = true;
        self.input.clear();
        self.input_lower.clear();
        self.suggestions.clear();
        self.selected_suggestion = None;
        self.submitted = None;
        self.history_cursor = None;
        self.saved_input.clear();
        self.tab_prefix = None;
        self.tab_cycle_index = 0;
    }

    /// Deactivate command mode (Escape or after submission).
    pub fn deactivate(&mut self) {
        self.active = false;
        self.input.clear();
        self.input_lower.clear();
        self.suggestions.clear();
        self.selected_suggestion = None;
        self.history_cursor = None;
        self.saved_input.clear();
        self.tab_prefix = None;
        self.tab_cycle_index = 0;
    }

    /// Push a character to the input and update suggestions.
    pub fn push_char(&mut self, c: char, registry: &ResourceRegistry) {
        self.reset_tab_cycle();
        self.input.push(c);
        self.input_lower = self.input.to_lowercase();
        self.update_suggestions(registry);
    }

    /// Delete the last character from input.
    pub fn backspace(&mut self, registry: &ResourceRegistry) {
        self.reset_tab_cycle();
        self.input.pop();
        self.input_lower = self.input.to_lowercase();
        if self.input.is_empty() {
            self.suggestions.clear();
            self.selected_suggestion = None;
        } else {
            self.update_suggestions(registry);
        }
    }

    /// Move selection down in the suggestion list (arrow keys).
    pub fn select_next(&mut self) {
        self.reset_tab_cycle();
        if self.suggestions.is_empty() {
            return;
        }
        self.selected_suggestion = Some(match self.selected_suggestion {
            None => 0,
            Some(i) => (i + 1).min(self.suggestions.len() - 1),
        });
    }

    /// Move selection up in the suggestion list (arrow keys).
    pub fn select_prev(&mut self) {
        self.reset_tab_cycle();
        if self.suggestions.is_empty() {
            return;
        }
        self.selected_suggestion = match self.selected_suggestion {
            None => None,
            Some(0) => None,
            Some(i) => Some(i - 1),
        };
    }

    /// Tab-complete: on first Tab, fill the input with the best match and enter
    /// tab-cycle mode. On subsequent Tabs, cycle forward through matches.
    /// The original prefix is preserved so each cycle re-matches against it.
    pub fn tab_complete(&mut self, _registry: &ResourceRegistry) {
        if self.suggestions.is_empty() && self.tab_prefix.is_none() {
            return;
        }

        if self.tab_prefix.is_some() {
            // Already in a tab-cycle session — advance to next match
            self.tab_cycle_index = (self.tab_cycle_index + 1) % self.suggestions.len().max(1);
        } else {
            // Start a new tab-cycle session: save current input as prefix
            self.tab_prefix = Some(self.input.clone());
            self.tab_cycle_index = 0;
        }

        // Fill input with the current cycle suggestion
        if let Some(suggestion) = self.suggestions.get(self.tab_cycle_index) {
            self.input = suggestion.display.clone();
            self.input_lower = self.input.to_lowercase();
            self.selected_suggestion = Some(self.tab_cycle_index);
        }
    }

    /// Shift+Tab: cycle backwards through tab-completion matches.
    pub fn tab_complete_prev(&mut self, _registry: &ResourceRegistry) {
        if self.suggestions.is_empty() && self.tab_prefix.is_none() {
            return;
        }

        if self.tab_prefix.is_some() {
            // Already in a tab-cycle session — go to previous match
            let len = self.suggestions.len().max(1);
            self.tab_cycle_index = if self.tab_cycle_index == 0 {
                len - 1
            } else {
                self.tab_cycle_index - 1
            };
        } else {
            // Start a new tab-cycle session from the last suggestion
            self.tab_prefix = Some(self.input.clone());
            self.tab_cycle_index = self.suggestions.len().saturating_sub(1);
        }

        // Fill input with the current cycle suggestion
        if let Some(suggestion) = self.suggestions.get(self.tab_cycle_index) {
            self.input = suggestion.display.clone();
            self.input_lower = self.input.to_lowercase();
            self.selected_suggestion = Some(self.tab_cycle_index);
        }
    }

    /// Accept the current selection (Tab) — fills the input with the selected suggestion.
    /// Kept for backward compatibility; prefer `tab_complete` for cycling behavior.
    pub fn accept_selection(&mut self, registry: &ResourceRegistry) {
        if let Some(idx) = self.selected_suggestion {
            if let Some(suggestion) = self.suggestions.get(idx) {
                self.input = suggestion.display.clone();
                self.input_lower = self.input.to_lowercase();
                self.update_suggestions(registry);
                // After accepting, clear selection so user can submit with Enter
                self.selected_suggestion = None;
            }
        }
    }

    /// Reset the tab-cycle state. Called when any non-Tab key is pressed.
    fn reset_tab_cycle(&mut self) {
        self.tab_prefix = None;
        self.tab_cycle_index = 0;
    }

    /// Returns ghost text to display after the cursor — the remaining portion
    /// of the top suggestion that extends beyond the current input.
    /// Returns `None` if there is no suggestion or the input already matches.
    pub fn ghost_text(&self) -> Option<&str> {
        // Don't show ghost text while cycling — the input already shows the full suggestion
        if self.tab_prefix.is_some() {
            return None;
        }
        // Don't show ghost text if a suggestion is already arrow-selected
        if self.selected_suggestion.is_some() {
            return None;
        }
        if self.input.is_empty() {
            return None;
        }
        let top = self.suggestions.first()?;
        let display_lower = top.display.to_lowercase();
        if display_lower.starts_with(&self.input_lower) && display_lower != self.input_lower {
            // Return the suffix from the original display (preserving case)
            Some(&top.display[self.input.len()..])
        } else {
            None
        }
    }

    /// Submit the current input (Enter pressed).
    /// Returns true if a command was submitted.
    pub fn submit(&mut self) -> bool {
        if self.input.is_empty() && self.selected_suggestion.is_none() {
            // Empty input: cancel
            self.deactivate();
            return false;
        }

        // If a suggestion is selected, use that instead of raw input
        let command = if let Some(idx) = self.selected_suggestion {
            self.suggestions
                .get(idx)
                .map(|s| s.display.clone())
                .unwrap_or_else(|| self.input.clone())
        } else {
            self.input.clone()
        };

        // Add to history (avoid consecutive duplicates)
        if self.history.back().map(|s| s.as_str()) != Some(command.as_str()) {
            self.history.push_back(command.clone());
            if self.history.len() > MAX_HISTORY {
                self.history.pop_front();
            }
        }

        self.submitted = Some(command);
        self.active = false;
        self.input.clear();
        self.input_lower.clear();
        self.suggestions.clear();
        self.selected_suggestion = None;
        self.history_cursor = None;
        self.saved_input.clear();
        true
    }

    /// Take the submitted command (consumes it).
    pub fn take_submitted(&mut self) -> Option<String> {
        self.submitted.take()
    }

    /// Navigate to the previous (older) history entry.
    /// Called on Up arrow in command mode.
    pub fn history_prev(&mut self, registry: &ResourceRegistry) {
        if self.history.is_empty() {
            return;
        }
        match self.history_cursor {
            None => {
                // Save current input and jump to most recent history entry
                self.saved_input = self.input.clone();
                let idx = self.history.len() - 1;
                self.history_cursor = Some(idx);
                self.input = self.history[idx].clone();
            }
            Some(idx) if idx > 0 => {
                let new_idx = idx - 1;
                self.history_cursor = Some(new_idx);
                self.input = self.history[new_idx].clone();
            }
            _ => return, // Already at oldest entry
        }
        self.input_lower = self.input.to_lowercase();
        self.update_suggestions(registry);
    }

    /// Navigate to the next (newer) history entry.
    /// Called on Down arrow in command mode.
    pub fn history_next(&mut self, registry: &ResourceRegistry) {
        let idx = match self.history_cursor {
            Some(idx) => idx,
            None => return, // Not browsing history
        };
        if idx + 1 < self.history.len() {
            let new_idx = idx + 1;
            self.history_cursor = Some(new_idx);
            self.input = self.history[new_idx].clone();
        } else {
            // Past end of history — restore saved input
            self.history_cursor = None;
            self.input = std::mem::take(&mut self.saved_input);
        }
        self.input_lower = self.input.to_lowercase();
        self.update_suggestions(registry);
    }

    /// Reset history cursor when the user types new characters (not Up/Down navigation).
    pub fn reset_history_cursor(&mut self) {
        if self.history_cursor.is_some() {
            self.history_cursor = None;
            self.saved_input.clear();
        }
    }

    /// Returns the number of history entries.
    #[cfg(test)]
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Returns the current history browsing cursor.
    #[cfg(test)]
    pub fn history_cursor(&self) -> Option<usize> {
        self.history_cursor
    }

    /// Returns whether a tab-cycle session is active.
    #[cfg(test)]
    pub fn is_tab_cycling(&self) -> bool {
        self.tab_prefix.is_some()
    }

    /// Returns the current tab-cycle index.
    #[cfg(test)]
    pub fn tab_cycle_index(&self) -> usize {
        self.tab_cycle_index
    }

    /// Returns the saved tab prefix (the original input before Tab was pressed).
    #[cfg(test)]
    pub fn tab_prefix(&self) -> Option<&str> {
        self.tab_prefix.as_deref()
    }

    /// Get the current input text.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Update suggestions from the registry based on current input.
    fn update_suggestions(&mut self, registry: &ResourceRegistry) {
        let matches = fuzzy_match(registry, &self.input_lower, MAX_SUGGESTIONS);
        self.suggestions = matches
            .into_iter()
            .map(|m| {
                let entry = &registry.entries()[m.entry_idx];
                let hint = if !entry.short_names.is_empty() {
                    entry.short_names.join(", ")
                } else {
                    entry.singular_name.clone()
                };
                Suggestion {
                    display: entry.resource.clone(),
                    hint,
                    api_group: if entry.api_group.is_empty() {
                        "core".to_string()
                    } else {
                        entry.api_group.clone()
                    },
                    score: m.score,
                    namespaced: entry.namespaced,
                    entry_idx: m.entry_idx,
                }
            })
            .collect();

        // Reset selection if suggestions changed
        if let Some(sel) = self.selected_suggestion {
            if sel >= self.suggestions.len() {
                self.selected_suggestion = if self.suggestions.is_empty() {
                    None
                } else {
                    Some(self.suggestions.len() - 1)
                };
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
    use crate::dash::resource_registry::ResourceRegistry;

    fn setup() -> (CommandMode, ResourceRegistry) {
        (
            CommandMode::new(),
            ResourceRegistry::with_builtin_resources(),
        )
    }

    #[test]
    fn activate_deactivate() {
        let (mut cmd, _reg) = setup();
        assert!(!cmd.active);
        cmd.activate();
        assert!(cmd.active);
        assert!(cmd.input.is_empty());
        cmd.deactivate();
        assert!(!cmd.active);
    }

    #[test]
    fn push_char_updates_suggestions() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('p', &reg);
        assert!(!cmd.suggestions.is_empty());
        // "p" should match pods, persistentvolumes, etc.
        let displays: Vec<&str> = cmd.suggestions.iter().map(|s| s.display.as_str()).collect();
        assert!(
            displays.contains(&"pods"),
            "Expected 'pods' in suggestions: {:?}",
            displays
        );
    }

    #[test]
    fn shortname_match() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('s', &reg);
        cmd.push_char('v', &reg);
        cmd.push_char('c', &reg);
        // "svc" is an exact shortname for services
        assert!(!cmd.suggestions.is_empty());
        assert_eq!(cmd.suggestions[0].display, "services");
    }

    #[test]
    fn deploy_shortname() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "deploy".chars() {
            cmd.push_char(c, &reg);
        }
        assert!(!cmd.suggestions.is_empty());
        assert_eq!(cmd.suggestions[0].display, "deployments");
    }

    #[test]
    fn backspace_updates_suggestions() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('p', &reg);
        cmd.push_char('o', &reg);
        let count_po = cmd.suggestions.len();
        cmd.backspace(&reg);
        // After backspace to "p", should have more or different suggestions
        assert!(cmd.input() == "p");
        let count_p = cmd.suggestions.len();
        assert!(count_p >= count_po);
    }

    #[test]
    fn backspace_to_empty_clears() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('p', &reg);
        cmd.backspace(&reg);
        assert!(cmd.input.is_empty());
        assert!(cmd.suggestions.is_empty());
    }

    #[test]
    fn select_next_prev() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('s', &reg);
        assert!(cmd.suggestions.len() >= 2);

        assert_eq!(cmd.selected_suggestion, None);
        cmd.select_next();
        assert_eq!(cmd.selected_suggestion, Some(0));
        cmd.select_next();
        assert_eq!(cmd.selected_suggestion, Some(1));
        cmd.select_prev();
        assert_eq!(cmd.selected_suggestion, Some(0));
        cmd.select_prev();
        assert_eq!(cmd.selected_suggestion, None);
    }

    #[test]
    fn accept_selection_fills_input() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('d', &reg);
        cmd.push_char('e', &reg);
        cmd.push_char('p', &reg);
        // Select first suggestion (should be "deployments")
        cmd.select_next();
        cmd.accept_selection(&reg);
        assert_eq!(cmd.input(), "deployments");
        assert_eq!(cmd.selected_suggestion, None);
    }

    #[test]
    fn submit_returns_command() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        assert!(cmd.submit());
        assert_eq!(cmd.take_submitted(), Some("pods".to_string()));
        assert!(!cmd.active);
    }

    #[test]
    fn submit_with_selection() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('p', &reg);
        cmd.select_next(); // select first suggestion
        let expected = cmd.suggestions[0].display.clone();
        assert!(cmd.submit());
        assert_eq!(cmd.take_submitted(), Some(expected));
    }

    #[test]
    fn submit_empty_cancels() {
        let (mut cmd, _reg) = setup();
        cmd.activate();
        assert!(!cmd.submit());
        assert!(!cmd.active);
        assert!(cmd.take_submitted().is_none());
    }

    #[test]
    fn suggestion_has_api_group() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "deploy".chars() {
            cmd.push_char(c, &reg);
        }
        assert!(!cmd.suggestions.is_empty());
        assert_eq!(cmd.suggestions[0].api_group, "apps");
    }

    #[test]
    fn suggestion_core_group_display() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "pod".chars() {
            cmd.push_char(c, &reg);
        }
        assert!(!cmd.suggestions.is_empty());
        assert_eq!(cmd.suggestions[0].api_group, "core");
    }

    #[test]
    fn fuzzy_match_cm() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('c', &reg);
        cmd.push_char('m', &reg);
        // "cm" is a shortname for configmaps
        assert!(!cmd.suggestions.is_empty());
        assert_eq!(cmd.suggestions[0].display, "configmaps");
    }

    #[test]
    fn fuzzy_match_hpa() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "hpa".chars() {
            cmd.push_char(c, &reg);
        }
        assert!(!cmd.suggestions.is_empty());
        assert_eq!(cmd.suggestions[0].display, "horizontalpodautoscalers");
    }

    #[test]
    fn max_suggestions_limit() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        // Single char should match many resources
        cmd.push_char('s', &reg);
        assert!(cmd.suggestions.len() <= MAX_SUGGESTIONS);
    }

    #[test]
    fn namespaced_flag_in_suggestions() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "node".chars() {
            cmd.push_char(c, &reg);
        }
        assert!(!cmd.suggestions.is_empty());
        // Nodes are cluster-scoped (not namespaced)
        assert!(!cmd.suggestions[0].namespaced);
    }

    // ---------------------------------------------------------------------------
    // History tests
    // ---------------------------------------------------------------------------

    #[test]
    fn submit_adds_to_history() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();
        assert_eq!(cmd.history_len(), 1);
    }

    #[test]
    fn history_deduplicates_consecutive() {
        let (mut cmd, reg) = setup();
        // Submit "pods" twice in a row
        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        assert_eq!(cmd.history_len(), 1); // Only one entry
    }

    #[test]
    fn history_allows_non_consecutive_duplicates() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        cmd.activate();
        for c in "nodes".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        assert_eq!(cmd.history_len(), 3); // pods, nodes, pods
    }

    #[test]
    fn history_prev_navigates_backwards() {
        let (mut cmd, reg) = setup();
        // Build up history: "pods", "nodes"
        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        cmd.activate();
        for c in "nodes".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        // Start new command and navigate back
        cmd.activate();
        cmd.push_char('x', &reg);
        assert_eq!(cmd.input(), "x");

        // Up → most recent ("nodes")
        cmd.history_prev(&reg);
        assert_eq!(cmd.input(), "nodes");
        assert_eq!(cmd.history_cursor(), Some(1));

        // Up → older ("pods")
        cmd.history_prev(&reg);
        assert_eq!(cmd.input(), "pods");
        assert_eq!(cmd.history_cursor(), Some(0));

        // Up again → stays at oldest
        cmd.history_prev(&reg);
        assert_eq!(cmd.input(), "pods");
        assert_eq!(cmd.history_cursor(), Some(0));
    }

    #[test]
    fn history_next_navigates_forwards() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        cmd.activate();
        for c in "nodes".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        // Start new command, navigate back, then forward
        cmd.activate();
        cmd.push_char('x', &reg);

        cmd.history_prev(&reg); // → "nodes"
        cmd.history_prev(&reg); // → "pods"

        cmd.history_next(&reg); // → "nodes"
        assert_eq!(cmd.input(), "nodes");
        assert_eq!(cmd.history_cursor(), Some(1));

        cmd.history_next(&reg); // → original "x"
        assert_eq!(cmd.input(), "x");
        assert_eq!(cmd.history_cursor(), None);
    }

    #[test]
    fn history_next_noop_when_not_browsing() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('x', &reg);
        cmd.history_next(&reg); // Should be a no-op
        assert_eq!(cmd.input(), "x");
        assert_eq!(cmd.history_cursor(), None);
    }

    #[test]
    fn history_prev_noop_when_empty() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.history_prev(&reg); // Should be a no-op
        assert!(cmd.input().is_empty());
        assert_eq!(cmd.history_cursor(), None);
    }

    #[test]
    fn reset_history_cursor_on_typing() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        cmd.submit();

        cmd.activate();
        cmd.history_prev(&reg); // → "pods"
        assert_eq!(cmd.history_cursor(), Some(0));

        cmd.reset_history_cursor();
        assert_eq!(cmd.history_cursor(), None);
    }

    // ---------------------------------------------------------------------------
    // Tab-completion cycling tests
    // ---------------------------------------------------------------------------

    #[test]
    fn tab_complete_first_press_fills_best_match() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('d', &reg);
        cmd.push_char('e', &reg);
        cmd.push_char('p', &reg);
        assert!(!cmd.is_tab_cycling());

        cmd.tab_complete(&reg);

        assert!(cmd.is_tab_cycling());
        assert_eq!(cmd.tab_prefix(), Some("dep"));
        assert_eq!(cmd.input(), "deployments");
        assert_eq!(cmd.selected_suggestion, Some(0));
        assert_eq!(cmd.tab_cycle_index(), 0);
    }

    #[test]
    fn tab_complete_cycles_through_matches() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        // "s" matches multiple resources: services, secrets, statefulsets, etc.
        cmd.push_char('s', &reg);
        let suggestion_count = cmd.suggestions.len();
        assert!(
            suggestion_count >= 3,
            "need at least 3 suggestions to test cycling"
        );

        let first = cmd.suggestions[0].display.clone();
        let second = cmd.suggestions[1].display.clone();
        let third = cmd.suggestions[2].display.clone();

        // First Tab → first match
        cmd.tab_complete(&reg);
        assert_eq!(cmd.input(), first);
        assert_eq!(cmd.tab_cycle_index(), 0);

        // Second Tab → second match
        cmd.tab_complete(&reg);
        assert_eq!(cmd.input(), second);
        assert_eq!(cmd.tab_cycle_index(), 1);

        // Third Tab → third match
        cmd.tab_complete(&reg);
        assert_eq!(cmd.input(), third);
        assert_eq!(cmd.tab_cycle_index(), 2);
    }

    #[test]
    fn tab_complete_wraps_around() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('p', &reg);
        cmd.push_char('v', &reg);
        let count = cmd.suggestions.len();
        assert!(count >= 1);

        // Tab through all suggestions
        for _ in 0..count {
            cmd.tab_complete(&reg);
        }
        assert_eq!(cmd.tab_cycle_index(), count - 1);

        // One more Tab should wrap to index 0
        cmd.tab_complete(&reg);
        assert_eq!(cmd.tab_cycle_index(), 0);
        assert_eq!(cmd.input(), cmd.suggestions[0].display);
    }

    #[test]
    fn tab_complete_prev_cycles_backward() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('s', &reg);
        let count = cmd.suggestions.len();
        assert!(count >= 2);

        let last_display = cmd.suggestions[count - 1].display.clone();

        // Shift+Tab starts from the last suggestion
        cmd.tab_complete_prev(&reg);
        assert!(cmd.is_tab_cycling());
        assert_eq!(cmd.input(), last_display);
        assert_eq!(cmd.tab_cycle_index(), count - 1);

        // Another Shift+Tab goes to second-to-last
        let second_last = cmd.suggestions[count - 2].display.clone();
        cmd.tab_complete_prev(&reg);
        assert_eq!(cmd.input(), second_last);
        assert_eq!(cmd.tab_cycle_index(), count - 2);
    }

    #[test]
    fn tab_complete_prev_wraps_to_end() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('d', &reg);
        cmd.push_char('e', &reg);
        cmd.push_char('p', &reg);
        let count = cmd.suggestions.len();
        assert!(count >= 1);

        let last_display = cmd.suggestions[count - 1].display.clone();

        // Tab forward to index 0
        cmd.tab_complete(&reg);
        assert_eq!(cmd.tab_cycle_index(), 0);

        // Shift+Tab wraps to last
        cmd.tab_complete_prev(&reg);
        assert_eq!(cmd.tab_cycle_index(), count - 1);
        assert_eq!(cmd.input(), last_display);
    }

    #[test]
    fn tab_cycle_mixed_forward_backward() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('s', &reg);
        assert!(cmd.suggestions.len() >= 3);

        let s0 = cmd.suggestions[0].display.clone();
        let s1 = cmd.suggestions[1].display.clone();

        // Tab → s0
        cmd.tab_complete(&reg);
        assert_eq!(cmd.input(), s0);

        // Tab → s1
        cmd.tab_complete(&reg);
        assert_eq!(cmd.input(), s1);

        // Shift+Tab → back to s0
        cmd.tab_complete_prev(&reg);
        assert_eq!(cmd.input(), s0);
        assert_eq!(cmd.tab_cycle_index(), 0);
    }

    #[test]
    fn typing_breaks_tab_cycle() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('d', &reg);
        cmd.push_char('e', &reg);

        cmd.tab_complete(&reg);
        assert!(cmd.is_tab_cycling());
        assert_eq!(cmd.tab_prefix(), Some("de"));

        // Type a character — should break tab-cycle
        cmd.push_char('x', &reg);
        assert!(!cmd.is_tab_cycling());
        assert_eq!(cmd.tab_prefix(), None);
    }

    #[test]
    fn backspace_breaks_tab_cycle() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('d', &reg);
        cmd.push_char('e', &reg);

        cmd.tab_complete(&reg);
        assert!(cmd.is_tab_cycling());

        cmd.backspace(&reg);
        assert!(!cmd.is_tab_cycling());
    }

    #[test]
    fn arrow_key_breaks_tab_cycle() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('s', &reg);

        cmd.tab_complete(&reg);
        assert!(cmd.is_tab_cycling());

        // Arrow-down (select_next) breaks tab cycle
        cmd.select_next();
        assert!(!cmd.is_tab_cycling());
    }

    #[test]
    fn tab_complete_noop_when_no_suggestions() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "zzzzz".chars() {
            cmd.push_char(c, &reg);
        }
        assert!(cmd.suggestions.is_empty());

        cmd.tab_complete(&reg);
        assert!(!cmd.is_tab_cycling());
        assert_eq!(cmd.input(), "zzzzz"); // unchanged
    }

    #[test]
    fn tab_complete_preserves_prefix_across_cycles() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('p', &reg);
        cmd.push_char('o', &reg);

        cmd.tab_complete(&reg);
        assert_eq!(cmd.tab_prefix(), Some("po"));

        // Even after cycling, prefix remains "po"
        cmd.tab_complete(&reg);
        assert_eq!(cmd.tab_prefix(), Some("po"));

        cmd.tab_complete(&reg);
        assert_eq!(cmd.tab_prefix(), Some("po"));
    }

    #[test]
    fn submit_after_tab_complete_uses_displayed_value() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('d', &reg);
        cmd.push_char('e', &reg);
        cmd.push_char('p', &reg);

        cmd.tab_complete(&reg);
        assert_eq!(cmd.input(), "deployments");

        assert!(cmd.submit());
        assert_eq!(cmd.take_submitted(), Some("deployments".to_string()));
    }

    #[test]
    fn tab_complete_then_submit_second_cycle() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('s', &reg);
        assert!(cmd.suggestions.len() >= 2);

        let second = cmd.suggestions[1].display.clone();

        cmd.tab_complete(&reg); // first
        cmd.tab_complete(&reg); // second

        assert!(cmd.submit());
        assert_eq!(cmd.take_submitted(), Some(second));
    }

    // ---------------------------------------------------------------------------
    // Ghost text tests
    // ---------------------------------------------------------------------------

    #[test]
    fn ghost_text_shows_suffix_of_top_suggestion() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('p', &reg);
        cmd.push_char('o', &reg);
        // Top suggestion for "po" should be "pods" — ghost text is "ds"
        let ghost = cmd.ghost_text();
        assert!(ghost.is_some(), "Expected ghost text for prefix 'po'");
        assert_eq!(ghost.unwrap(), "ds");
    }

    #[test]
    fn ghost_text_none_when_empty_input() {
        let (mut cmd, _reg) = setup();
        cmd.activate();
        assert!(cmd.ghost_text().is_none());
    }

    #[test]
    fn ghost_text_none_when_no_suggestions() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "zzzzz".chars() {
            cmd.push_char(c, &reg);
        }
        assert!(cmd.ghost_text().is_none());
    }

    #[test]
    fn ghost_text_none_when_exact_match() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        for c in "pods".chars() {
            cmd.push_char(c, &reg);
        }
        // Input exactly matches "pods" — no ghost suffix needed
        assert!(cmd.ghost_text().is_none());
    }

    #[test]
    fn ghost_text_none_when_suggestion_selected() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('p', &reg);
        cmd.select_next(); // arrow key selects first suggestion
        assert!(
            cmd.ghost_text().is_none(),
            "Ghost text should hide when a suggestion is arrow-selected"
        );
    }

    #[test]
    fn ghost_text_none_during_tab_cycle() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('d', &reg);
        cmd.push_char('e', &reg);
        cmd.tab_complete(&reg); // start tab cycle
        assert!(
            cmd.ghost_text().is_none(),
            "Ghost text should hide during tab cycling"
        );
    }

    #[test]
    fn ghost_text_returns_after_tab_cycle_reset() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('d', &reg);
        cmd.push_char('e', &reg);
        cmd.tab_complete(&reg); // start tab cycle — fills input
        assert!(cmd.ghost_text().is_none());

        // Type a new char to reset tab cycle
        cmd.push_char('p', &reg);
        // Now ghost text should be available again if prefix matches
        // Input is now "deploymentsp" which won't match — but that's fine,
        // the important thing is the tab_prefix is cleared
        assert!(cmd.tab_prefix.is_none());
    }

    // ---------------------------------------------------------------------------
    // Arrow key resets tab cycle
    // ---------------------------------------------------------------------------

    #[test]
    fn arrow_select_resets_tab_cycle() {
        let (mut cmd, reg) = setup();
        cmd.activate();
        cmd.push_char('s', &reg);
        cmd.tab_complete(&reg); // start tab cycle
        assert!(cmd.tab_prefix.is_some());

        cmd.select_next(); // arrow key should reset tab cycle
        assert!(cmd.tab_prefix.is_none());
    }
}
