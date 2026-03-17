/// Keybinding registry: maps each TUI mode to its keybinding entries.
///
/// Used by the help overlay to display context-sensitive keybinding reference,
/// and as the single source of truth for all keybinding documentation.
///
/// A TUI interaction mode that has its own set of keybindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Resource list / table view (main dashboard view)
    ListView,
    /// YAML editor — normal mode (vim-style motions, commands)
    EditorNormal,
    /// YAML editor — insert mode (typing text)
    EditorInsert,
    /// Log viewer (streaming pod logs)
    LogViewer,
    /// Port-forward manager (list and manage active forwards)
    PortForwardManager,
}

impl Mode {
    /// Human-readable label for the mode.
    pub fn label(&self) -> &'static str {
        match self {
            Mode::ListView => "List View",
            Mode::EditorNormal => "Editor (Normal)",
            Mode::EditorInsert => "Editor (Insert)",
            Mode::LogViewer => "Log Viewer",
            Mode::PortForwardManager => "Port Forwards",
        }
    }

    /// All modes in display order.
    pub fn all() -> &'static [Mode] {
        &[
            Mode::ListView,
            Mode::EditorNormal,
            Mode::EditorInsert,
            Mode::LogViewer,
            Mode::PortForwardManager,
        ]
    }
}

/// Category grouping for keybinding entries within a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Navigation,
    Actions,
    ResourceSwitch,
    Search,
    CommandMode,
    Editing,
    Selection,
    Clipboard,
    UndoRedo,
    Global,
}

impl Category {
    /// Human-readable section heading.
    pub fn label(&self) -> &'static str {
        match self {
            Category::Navigation => "Navigation",
            Category::Actions => "Actions",
            Category::ResourceSwitch => "Resource Switch",
            Category::Search => "Search",
            Category::CommandMode => "Command Mode",
            Category::Editing => "Editing",
            Category::Selection => "Selection",
            Category::Clipboard => "Clipboard",
            Category::UndoRedo => "Undo / Redo",
            Category::Global => "Global",
        }
    }
}

/// A single keybinding entry: what key(s) to press, what it does, and its category.
#[derive(Debug, Clone)]
pub struct KeybindingEntry {
    /// Display string for the key combo (e.g. "j/k", "Ctrl+C", "dd").
    pub key: &'static str,
    /// Short human-readable description.
    pub description: &'static str,
    /// Grouping category for rendering sections.
    pub category: Category,
}

/// Registry mapping each [`Mode`] to its ordered list of [`KeybindingEntry`] values.
pub struct KeybindingRegistry {
    entries: Vec<(Mode, Vec<KeybindingEntry>)>,
}

impl KeybindingRegistry {
    /// Build the registry with all modes populated.
    pub fn new() -> Self {
        Self {
            entries: vec![
                (Mode::ListView, Self::list_view_bindings()),
                (Mode::EditorNormal, Self::editor_normal_bindings()),
                (Mode::EditorInsert, Self::editor_insert_bindings()),
                (Mode::LogViewer, Self::log_viewer_bindings()),
                (Mode::PortForwardManager, Self::port_forward_bindings()),
            ],
        }
    }

    /// Get keybinding entries for a given mode.
    pub fn get(&self, mode: Mode) -> &[KeybindingEntry] {
        self.entries
            .iter()
            .find(|(m, _)| *m == mode)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[])
    }

    /// Iterate over all (mode, entries) pairs.
    pub fn iter(&self) -> impl Iterator<Item = &(Mode, Vec<KeybindingEntry>)> {
        self.entries.iter()
    }

    // -----------------------------------------------------------------------
    // Mode-specific binding definitions
    // -----------------------------------------------------------------------

    fn list_view_bindings() -> Vec<KeybindingEntry> {
        vec![
            // Navigation
            KeybindingEntry {
                key: "j / ↓",
                description: "Move cursor down",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "k / ↑",
                description: "Move cursor up",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "h / ←",
                description: "Collapse tree node / move left",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "l / →",
                description: "Expand tree node / move right",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "PgUp / PgDn",
                description: "Jump half page up/down",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "Home / End",
                description: "Jump to first / last item",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "Enter",
                description: "Select cluster/namespace or describe resource",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "Tab",
                description: "Switch panel (Sidebar ↔ Center)",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "Shift+Tab",
                description: "Switch panel (reverse)",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "1 / 2",
                description: "Switch tab (Resources / Top)",
                category: Category::Navigation,
            },
            // Actions
            KeybindingEntry {
                key: "e",
                description: "Edit resource YAML",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "d",
                description: "Describe resource (YAML view)",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "Ctrl+d",
                description: "Delete resource (with confirmation)",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "l",
                description: "View logs (pods only)",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "s",
                description: "Shell exec into pod",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "Shift+f",
                description: "Port-forward selected pod/service",
                category: Category::Actions,
            },
            // Resource switch
            KeybindingEntry {
                key: "p",
                description: "Switch to Pods view",
                category: Category::ResourceSwitch,
            },
            // Search
            KeybindingEntry {
                key: "/",
                description: "Start fuzzy filter",
                category: Category::Search,
            },
            KeybindingEntry {
                key: "ESC",
                description: "Clear filter / close overlay",
                category: Category::Search,
            },
            // Command mode
            KeybindingEntry {
                key: ":",
                description: "Open command mode (e.g. :deploy :svc :crd)",
                category: Category::CommandMode,
            },
            // Global
            KeybindingEntry {
                key: "r",
                description: "Force refresh",
                category: Category::Global,
            },
            KeybindingEntry {
                key: "?",
                description: "Toggle help overlay",
                category: Category::Global,
            },
            KeybindingEntry {
                key: "q",
                description: "Quit",
                category: Category::Global,
            },
            KeybindingEntry {
                key: "Ctrl+c",
                description: "Force quit (always works)",
                category: Category::Global,
            },
        ]
    }

    fn editor_normal_bindings() -> Vec<KeybindingEntry> {
        vec![
            // Navigation
            KeybindingEntry {
                key: "h / j / k / l",
                description: "Move cursor left/down/up/right",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "w / b",
                description: "Next / previous word",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "0 / $",
                description: "Start / end of line",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "gg / G",
                description: "Go to first / last line",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "Ctrl+u / Ctrl+d",
                description: "Half-page up / down",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "{ / }",
                description: "Previous / next blank line",
                category: Category::Navigation,
            },
            // Editing
            KeybindingEntry {
                key: "i",
                description: "Enter insert mode (before cursor)",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "a",
                description: "Enter insert mode (after cursor)",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "o / O",
                description: "Open line below / above",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "x",
                description: "Delete character under cursor",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "dd",
                description: "Delete (cut) current line",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "D",
                description: "Delete to end of line",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "cc",
                description: "Change (replace) current line",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "C",
                description: "Change to end of line",
                category: Category::Editing,
            },
            // Clipboard
            KeybindingEntry {
                key: "yy",
                description: "Yank (copy) current line",
                category: Category::Clipboard,
            },
            KeybindingEntry {
                key: "p / P",
                description: "Paste after / before cursor",
                category: Category::Clipboard,
            },
            // Undo / Redo
            KeybindingEntry {
                key: "u",
                description: "Undo",
                category: Category::UndoRedo,
            },
            KeybindingEntry {
                key: "Ctrl+r",
                description: "Redo",
                category: Category::UndoRedo,
            },
            // Actions
            KeybindingEntry {
                key: ":w",
                description: "Save (server-side apply)",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: ":q",
                description: "Quit editor (discard if unchanged)",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: ":wq",
                description: "Save and quit editor",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: ":q!",
                description: "Force quit (discard changes)",
                category: Category::Actions,
            },
            // Search
            KeybindingEntry {
                key: "/",
                description: "Search forward in document",
                category: Category::Search,
            },
            KeybindingEntry {
                key: "n / N",
                description: "Next / previous search match",
                category: Category::Search,
            },
            // Global
            KeybindingEntry {
                key: "ESC",
                description: "Return to list view (if unchanged)",
                category: Category::Global,
            },
        ]
    }

    fn editor_insert_bindings() -> Vec<KeybindingEntry> {
        vec![
            // Editing
            KeybindingEntry {
                key: "<type>",
                description: "Insert characters at cursor",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "Backspace",
                description: "Delete character before cursor",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "Delete",
                description: "Delete character under cursor",
                category: Category::Editing,
            },
            KeybindingEntry {
                key: "Enter",
                description: "Insert newline",
                category: Category::Editing,
            },
            // Navigation
            KeybindingEntry {
                key: "← / → / ↑ / ↓",
                description: "Move cursor",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "Home / End",
                description: "Start / end of line",
                category: Category::Navigation,
            },
            // Global
            KeybindingEntry {
                key: "ESC",
                description: "Return to normal mode",
                category: Category::Global,
            },
        ]
    }

    fn log_viewer_bindings() -> Vec<KeybindingEntry> {
        vec![
            // Navigation
            KeybindingEntry {
                key: "j / ↓",
                description: "Scroll down one line",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "k / ↑",
                description: "Scroll up one line",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "PgUp / PgDn",
                description: "Scroll half page up/down",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "g / G",
                description: "Jump to top / bottom",
                category: Category::Navigation,
            },
            // Actions
            KeybindingEntry {
                key: "f",
                description: "Toggle auto-follow (tail mode)",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "w",
                description: "Toggle line wrap",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "t",
                description: "Toggle timestamps",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "p",
                description: "Toggle previous container logs",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "c",
                description: "Switch container (if multi-container pod)",
                category: Category::Actions,
            },
            // Search
            KeybindingEntry {
                key: "/",
                description: "Search / filter log lines",
                category: Category::Search,
            },
            KeybindingEntry {
                key: "n / N",
                description: "Next / previous match",
                category: Category::Search,
            },
            // Global
            KeybindingEntry {
                key: "q / ESC",
                description: "Close log viewer",
                category: Category::Global,
            },
            KeybindingEntry {
                key: "Ctrl+c",
                description: "Force quit (always works)",
                category: Category::Global,
            },
        ]
    }

    fn port_forward_bindings() -> Vec<KeybindingEntry> {
        vec![
            // Navigation
            KeybindingEntry {
                key: "j / ↓",
                description: "Move cursor down",
                category: Category::Navigation,
            },
            KeybindingEntry {
                key: "k / ↑",
                description: "Move cursor up",
                category: Category::Navigation,
            },
            // Actions
            KeybindingEntry {
                key: "Enter",
                description: "Edit selected port-forward",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "d",
                description: "Delete (stop) selected port-forward",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "a",
                description: "Add new port-forward",
                category: Category::Actions,
            },
            KeybindingEntry {
                key: "r",
                description: "Restart selected port-forward",
                category: Category::Actions,
            },
            // Global
            KeybindingEntry {
                key: "q / ESC",
                description: "Close port-forward manager",
                category: Category::Global,
            },
            KeybindingEntry {
                key: "Ctrl+c",
                description: "Force quit (always works)",
                category: Category::Global,
            },
        ]
    }
}

impl Default for KeybindingRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_modes_have_entries() {
        let registry = KeybindingRegistry::new();
        for mode in Mode::all() {
            let entries = registry.get(*mode);
            assert!(
                !entries.is_empty(),
                "Mode {:?} should have at least one keybinding entry",
                mode
            );
        }
    }

    #[test]
    fn all_modes_have_global_category() {
        let registry = KeybindingRegistry::new();
        for mode in Mode::all() {
            let entries = registry.get(*mode);
            let has_global = entries.iter().any(|e| e.category == Category::Global);
            assert!(
                has_global,
                "Mode {:?} should have at least one Global keybinding (quit/escape)",
                mode
            );
        }
    }

    #[test]
    fn list_view_has_expected_sections() {
        let registry = KeybindingRegistry::new();
        let entries = registry.get(Mode::ListView);
        let categories: Vec<Category> = entries.iter().map(|e| e.category).collect();
        assert!(categories.contains(&Category::Navigation));
        assert!(categories.contains(&Category::Actions));
        assert!(categories.contains(&Category::Search));
        assert!(categories.contains(&Category::CommandMode));
        assert!(categories.contains(&Category::Global));
    }

    #[test]
    fn editor_normal_has_vim_motions() {
        let registry = KeybindingRegistry::new();
        let entries = registry.get(Mode::EditorNormal);
        let keys: Vec<&str> = entries.iter().map(|e| e.key).collect();
        assert!(keys.contains(&"h / j / k / l"));
        assert!(keys.contains(&"w / b"));
        assert!(keys.contains(&"dd"));
        assert!(keys.contains(&"yy"));
        assert!(keys.contains(&"u"));
        assert!(keys.contains(&"Ctrl+r"));
    }

    #[test]
    fn editor_insert_returns_to_normal_on_esc() {
        let registry = KeybindingRegistry::new();
        let entries = registry.get(Mode::EditorInsert);
        let esc_entry = entries
            .iter()
            .find(|e| e.key == "ESC")
            .expect("EditorInsert must have ESC binding");
        assert!(esc_entry.description.contains("normal mode"));
    }

    #[test]
    fn log_viewer_has_follow_toggle() {
        let registry = KeybindingRegistry::new();
        let entries = registry.get(Mode::LogViewer);
        let has_follow = entries.iter().any(|e| e.key == "f" && e.description.contains("follow"));
        assert!(has_follow, "LogViewer should have follow toggle on 'f'");
    }

    #[test]
    fn port_forward_has_add_delete() {
        let registry = KeybindingRegistry::new();
        let entries = registry.get(Mode::PortForwardManager);
        let keys: Vec<&str> = entries.iter().map(|e| e.key).collect();
        assert!(keys.contains(&"a"), "PortForwardManager should have 'a' for add");
        assert!(keys.contains(&"d"), "PortForwardManager should have 'd' for delete");
    }

    #[test]
    fn no_duplicate_keys_within_same_category() {
        let registry = KeybindingRegistry::new();
        for mode in Mode::all() {
            let entries = registry.get(*mode);
            // Within each category, keys should be unique
            let mut seen = std::collections::HashSet::new();
            for entry in entries {
                let combo = (entry.category, entry.key);
                assert!(
                    seen.insert(combo),
                    "Duplicate key {:?} in category {:?} for mode {:?}",
                    entry.key,
                    entry.category,
                    mode
                );
            }
        }
    }

    #[test]
    fn mode_labels_non_empty() {
        for mode in Mode::all() {
            assert!(!mode.label().is_empty());
        }
    }

    #[test]
    fn category_labels_non_empty() {
        let cats = [
            Category::Navigation,
            Category::Actions,
            Category::ResourceSwitch,
            Category::Search,
            Category::CommandMode,
            Category::Editing,
            Category::Selection,
            Category::Clipboard,
            Category::UndoRedo,
            Category::Global,
        ];
        for cat in cats {
            assert!(!cat.label().is_empty());
        }
    }

    #[test]
    fn default_impl_works() {
        let registry = KeybindingRegistry::default();
        assert!(!registry.get(Mode::ListView).is_empty());
    }
}
