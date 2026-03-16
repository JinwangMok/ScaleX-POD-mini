use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Quit,
    // Navigation
    Up,
    Down,
    Left,
    Right,
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
    // Refresh
    Refresh, // r
    // Tick (periodic refresh)
    Tick,
    // No event
    None,
}

/// Poll for input events with a timeout.
/// Returns `AppEvent::Tick` if no input within timeout.
pub fn poll_event(tick_rate: Duration) -> Result<AppEvent> {
    if event::poll(tick_rate)? {
        if let Event::Key(key) = event::read()? {
            return Ok(map_key_event(key));
        }
    }
    Ok(AppEvent::Tick)
}

fn map_key_event(key: KeyEvent) -> AppEvent {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    match key.code {
        // Quit
        KeyCode::Char('q') if !ctrl => AppEvent::Quit,
        KeyCode::Char('c') if ctrl => AppEvent::Quit,

        // Ctrl+1..9 → tab switch
        KeyCode::Char(c @ '1'..='9') if ctrl => AppEvent::Tab(c.to_digit(10).unwrap() as usize),

        // Navigation
        KeyCode::Up | KeyCode::Char('k') => AppEvent::Up,
        KeyCode::Down | KeyCode::Char('j') => AppEvent::Down,
        KeyCode::Left | KeyCode::Char('h') => AppEvent::Left,
        KeyCode::Right | KeyCode::Char('l') => AppEvent::Right,
        KeyCode::Enter => AppEvent::Enter,
        KeyCode::Backspace => AppEvent::Left,

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
    fn ctrl_c_quits() {
        assert_eq!(
            map_key_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            AppEvent::Quit
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
}
