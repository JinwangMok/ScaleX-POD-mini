use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Quit,
    ForceQuit, // Ctrl+C — always quits, even in search/help mode
    // Navigation (vim keys: j/k/h/l — typed as chars in search mode)
    Up,
    Down,
    Left,
    Right,
    // Navigation (arrow keys — never typed as chars in search mode)
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Enter,
    // Tab switching
    Tab(usize), // Ctrl+1..9
    NextPanel,  // Tab key
    PrevPanel,  // Shift+Tab
    // Resource type switch in center panel
    ResourceType(char), // p, d, s, c, n
    // Search
    Search, // /
    // Help
    Help, // ?
    // Escape (close overlay / cancel)
    Escape,
    // Backspace (separate from Left for search mode)
    Backspace,
    // Refresh
    Refresh, // r
    // Page navigation (half-viewport jumps)
    PageUp,
    PageDown,
    // Jump to first/last item
    Home,
    End,
    // Character input (unmapped printable chars — used by search mode)
    CharInput(char),
    // Tick (periodic refresh)
    Tick,
    // No event
    None,
}

/// Poll for input events with a timeout.
/// Returns `AppEvent::Tick` if no input within timeout.
pub fn poll_event(tick_rate: Duration) -> Result<AppEvent> {
    if event::poll(tick_rate)? {
        match event::read()? {
            Event::Key(key) => return Ok(map_key_event(key)),
            Event::Resize(_, _) => {
                // Terminal resized — return Tick to trigger immediate re-render
                return Ok(AppEvent::Tick);
            }
            _ => {}
        }
    }
    Ok(AppEvent::Tick)
}

fn map_key_event(key: KeyEvent) -> AppEvent {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    match key.code {
        // Force quit (always exits, even in search/help mode)
        KeyCode::Char('c') if ctrl => AppEvent::ForceQuit,

        // Quit
        KeyCode::Char('q') if !ctrl => AppEvent::Quit,

        // Ctrl+1..9 → tab switch
        KeyCode::Char(c @ '1'..='9') if ctrl => AppEvent::Tab(c.to_digit(10).unwrap() as usize),

        // Navigation — arrow keys (distinct from vim keys for search mode)
        KeyCode::Up if !ctrl => AppEvent::ArrowUp,
        KeyCode::Down if !ctrl => AppEvent::ArrowDown,
        KeyCode::Left if !ctrl => AppEvent::ArrowLeft,
        KeyCode::Right if !ctrl => AppEvent::ArrowRight,
        // Page navigation
        KeyCode::PageUp => AppEvent::PageUp,
        KeyCode::PageDown => AppEvent::PageDown,
        KeyCode::Home => AppEvent::Home,
        KeyCode::End => AppEvent::End,
        // Navigation — vim keys (typed as chars in search mode)
        KeyCode::Char('k') if !ctrl => AppEvent::Up,
        KeyCode::Char('j') if !ctrl => AppEvent::Down,
        KeyCode::Char('h') if !ctrl => AppEvent::Left,
        KeyCode::Char('l') if !ctrl => AppEvent::Right,
        KeyCode::Enter => AppEvent::Enter,
        KeyCode::Backspace => AppEvent::Backspace,

        // Panel cycling
        KeyCode::Tab if shift => AppEvent::PrevPanel,
        KeyCode::Tab => AppEvent::NextPanel,
        KeyCode::BackTab => AppEvent::PrevPanel,

        // Resource type shortcuts (center panel)
        KeyCode::Char('p') if !ctrl => AppEvent::ResourceType('p'),
        KeyCode::Char('d') if !ctrl => AppEvent::ResourceType('d'),
        KeyCode::Char('s') if !ctrl => AppEvent::ResourceType('s'),
        KeyCode::Char('c') if !ctrl => AppEvent::ResourceType('c'),
        KeyCode::Char('n') if !ctrl => AppEvent::ResourceType('n'),

        // Search & Help
        KeyCode::Char('/') => AppEvent::Search,
        KeyCode::Char('?') => AppEvent::Help,
        KeyCode::Esc => AppEvent::Escape,

        // Refresh
        KeyCode::Char('r') if !ctrl => AppEvent::Refresh,

        // Any other printable character (for search mode input)
        KeyCode::Char(c) if !ctrl => AppEvent::CharInput(c),

        _ => AppEvent::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn quit_on_q() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('q'), KeyModifiers::NONE)),
            AppEvent::Quit
        );
    }

    #[test]
    fn ctrl_c_force_quits() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            AppEvent::ForceQuit
        );
    }

    #[test]
    fn ctrl_1_tab() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('1'), KeyModifiers::CONTROL)),
            AppEvent::Tab(1)
        );
    }

    #[test]
    fn j_moves_down() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('j'), KeyModifiers::NONE)),
            AppEvent::Down
        );
    }

    #[test]
    fn arrow_keys_map_to_arrow_events() {
        assert_eq!(
            map_key_event(key(KeyCode::Up, KeyModifiers::NONE)),
            AppEvent::ArrowUp
        );
        assert_eq!(
            map_key_event(key(KeyCode::Down, KeyModifiers::NONE)),
            AppEvent::ArrowDown
        );
        assert_eq!(
            map_key_event(key(KeyCode::Left, KeyModifiers::NONE)),
            AppEvent::ArrowLeft
        );
        assert_eq!(
            map_key_event(key(KeyCode::Right, KeyModifiers::NONE)),
            AppEvent::ArrowRight
        );
    }

    #[test]
    fn slash_search() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('/'), KeyModifiers::NONE)),
            AppEvent::Search
        );
    }

    #[test]
    fn esc_maps_to_escape() {
        assert_eq!(
            map_key_event(key(KeyCode::Esc, KeyModifiers::NONE)),
            AppEvent::Escape
        );
    }

    #[test]
    fn unmapped_char_becomes_char_input() {
        // 'a' is not mapped to any specific event
        assert_eq!(
            map_key_event(key(KeyCode::Char('a'), KeyModifiers::NONE)),
            AppEvent::CharInput('a')
        );
    }

    #[test]
    fn unmapped_chars_various() {
        for c in [
            'b', 'e', 'f', 'g', 'i', 'm', 'o', 't', 'u', 'w', 'x', 'y', 'z',
        ] {
            assert_eq!(
                map_key_event(key(KeyCode::Char(c), KeyModifiers::NONE)),
                AppEvent::CharInput(c),
                "char '{}' should map to CharInput",
                c
            );
        }
    }

    #[test]
    fn mapped_chars_not_char_input() {
        // These should map to their specific events, not CharInput
        assert_eq!(
            map_key_event(key(KeyCode::Char('q'), KeyModifiers::NONE)),
            AppEvent::Quit
        );
        assert_eq!(
            map_key_event(key(KeyCode::Char('j'), KeyModifiers::NONE)),
            AppEvent::Down
        );
        assert_eq!(
            map_key_event(key(KeyCode::Char('p'), KeyModifiers::NONE)),
            AppEvent::ResourceType('p')
        );
    }
}
