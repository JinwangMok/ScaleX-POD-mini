//! TUI suspend/restore logic for shell exec.
//!
//! When the user wants to run an external command (e.g. `kubectl exec -it ...`),
//! we must leave the alternate screen, disable raw mode, and hand the terminal
//! back to the child process.  After the child exits we restore the TUI state.

use anyhow::Result;
use crossterm::{
    cursor,
    execute,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of running an external command while the TUI was suspended.
#[derive(Debug, Clone)]
pub struct SuspendResult {
    /// Exit code returned by the child process, if it exited normally.
    pub exit_code: Option<i32>,
    /// Human-readable error string if the spawn/wait failed.
    pub error: Option<String>,
}

/// Request queued from the event handler to the `run_tui` loop so that the
/// outer loop can call [`run_with_suspended_tui`] *outside* the borrow of
/// `AppState`.
///
/// Uses kube-rs exec API (no kubectl dependency) — the run_tui loop suspends
/// the TUI, starts an exec session via WebSocket, bridges I/O, then restores.
#[derive(Debug, Clone)]
pub struct ShellExecRequest {
    /// Pod name to exec into.
    pub pod_name: String,
    /// Pod namespace.
    pub namespace: String,
    /// Target container (None = default/only container).
    pub container: Option<String>,
    /// Short human-readable description shown before/after the command runs.
    pub description: String,
}

// ---------------------------------------------------------------------------
// Suspend / Restore
// ---------------------------------------------------------------------------

/// Tear down TUI state so an external process can use the terminal.
///
/// - Shows the cursor
/// - Leaves the alternate screen
/// - Disables raw mode
pub fn suspend_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    execute!(
        terminal.backend_mut(),
        cursor::Show,
        LeaveAlternateScreen
    )?;
    terminal::disable_raw_mode()?;
    Ok(())
}

/// Re-establish TUI state after an external process has finished.
///
/// - Enables raw mode
/// - Enters the alternate screen
/// - Hides the cursor
/// - Clears the terminal so the next `draw()` starts clean
pub fn restore_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    terminal::enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        cursor::Hide,
        Clear(ClearType::All)
    )?;
    // Force ratatui to repaint every cell on the next draw.
    terminal.clear()?;
    Ok(())
}

// Note: exec sessions are handled directly in the run_tui loop using kube-rs
// WebSocket exec API, calling suspend_tui/restore_tui around the session.

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- SuspendResult -------------------------------------------------------

    #[test]
    fn suspend_result_success() {
        let r = SuspendResult {
            exit_code: Some(0),
            error: None,
        };
        assert_eq!(r.exit_code, Some(0));
        assert!(r.error.is_none());
    }

    #[test]
    fn suspend_result_with_error() {
        let r = SuspendResult {
            exit_code: None,
            error: Some("boom".into()),
        };
        assert!(r.exit_code.is_none());
        assert_eq!(r.error.as_deref(), Some("boom"));
    }

    #[test]
    fn suspend_result_nonzero_exit() {
        let r = SuspendResult {
            exit_code: Some(127),
            error: None,
        };
        assert_eq!(r.exit_code, Some(127));
        assert!(r.error.is_none());
    }

    #[test]
    fn suspend_result_clone() {
        let r = SuspendResult {
            exit_code: Some(1),
            error: Some("err".into()),
        };
        let r2 = r.clone();
        assert_eq!(r.exit_code, r2.exit_code);
        assert_eq!(r.error, r2.error);
    }

    #[test]
    fn suspend_result_debug() {
        let r = SuspendResult {
            exit_code: Some(0),
            error: None,
        };
        let dbg = format!("{r:?}");
        assert!(dbg.contains("SuspendResult"));
        assert!(dbg.contains("exit_code"));
    }

    // -- ShellExecRequest ----------------------------------------------------

    #[test]
    fn shell_exec_request_fields() {
        let req = ShellExecRequest {
            pod_name: "my-pod".into(),
            namespace: "default".into(),
            container: Some("nginx".into()),
            description: "Shell into default/my-pod".into(),
        };
        assert_eq!(req.pod_name, "my-pod");
        assert_eq!(req.namespace, "default");
        assert_eq!(req.container.as_deref(), Some("nginx"));
        assert_eq!(req.description, "Shell into default/my-pod");
    }

    #[test]
    fn shell_exec_request_clone() {
        let req = ShellExecRequest {
            pod_name: "pod-1".into(),
            namespace: "kube-system".into(),
            container: None,
            description: "exec test".into(),
        };
        let req2 = req.clone();
        assert_eq!(req.pod_name, req2.pod_name);
        assert_eq!(req.namespace, req2.namespace);
        assert_eq!(req.container, req2.container);
        assert_eq!(req.description, req2.description);
    }

    #[test]
    fn shell_exec_request_debug() {
        let req = ShellExecRequest {
            pod_name: "debug-pod".into(),
            namespace: "dev".into(),
            container: Some("app".into()),
            description: "debug test".into(),
        };
        let dbg = format!("{req:?}");
        assert!(dbg.contains("ShellExecRequest"));
        assert!(dbg.contains("debug-pod"));
    }

    #[test]
    fn shell_exec_request_no_container() {
        let req = ShellExecRequest {
            pod_name: "single".into(),
            namespace: "default".into(),
            container: None,
            description: "single container".into(),
        };
        assert!(req.container.is_none());
    }

    // -- suspend_tui / restore_tui -------------------------------------------
    // These require a real terminal so we cannot call them directly in CI.
    // Instead we verify the functions exist with the expected signatures by
    // taking references to them.

    #[test]
    fn suspend_restore_fn_signatures() {
        // Compile-time proof that the public API exists with the right types.
        let _s: fn(&mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> = suspend_tui;
        let _r: fn(&mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> = restore_tui;
    }

}
