//! Pod exec session manager for the TUI dashboard.
//!
//! Provides an `ExecSession` abstraction that spawns a tokio task to drive
//! kube-rs `exec()` via WebSocket, bridging stdin/stdout/stderr through
//! channels that the TUI event loop can poll without blocking.

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// ExecRequest — describes what to exec into
// ---------------------------------------------------------------------------

/// Describes a pod exec request before a session is started.
#[derive(Debug, Clone)]
pub struct ExecRequest {
    pub pod_name: String,
    pub namespace: String,
    pub container: Option<String>,
    pub command: Vec<String>,
    pub tty: bool,
}

impl ExecRequest {
    /// Create a request that opens an interactive shell in the pod.
    /// The actual shell binary is resolved at connect time by trying
    /// `/bin/bash` first, then falling back to `/bin/sh`.
    pub fn shell(pod_name: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            pod_name: pod_name.into(),
            namespace: namespace.into(),
            container: None,
            command: Vec::new(), // empty = try shell candidates
            tty: true,
        }
    }

    /// Create a request that runs a specific command in the pod.
    pub fn with_command(
        pod_name: impl Into<String>,
        namespace: impl Into<String>,
        command: Vec<String>,
    ) -> Self {
        Self {
            pod_name: pod_name.into(),
            namespace: namespace.into(),
            container: None,
            command,
            tty: false,
        }
    }

    /// Set the target container (builder pattern).
    pub fn container(mut self, name: impl Into<String>) -> Self {
        self.container = Some(name.into());
        self
    }

    /// Override the tty flag (builder pattern).
    pub fn tty(mut self, tty: bool) -> Self {
        self.tty = tty;
        self
    }
}

// ---------------------------------------------------------------------------
// ExecMessage — messages produced by the background task
// ---------------------------------------------------------------------------

/// Messages flowing from the exec background task to the TUI.
#[derive(Debug, Clone)]
pub enum ExecMessage {
    /// Data received on stdout.
    Stdout(Vec<u8>),
    /// Data received on stderr.
    Stderr(Vec<u8>),
    /// Process exited. The optional string carries an error/status description
    /// when the exit was abnormal.
    Exited(Option<String>),
    /// An error occurred in the exec plumbing.
    Error(String),
}

// ---------------------------------------------------------------------------
// Resize request sent from TUI -> background task
// ---------------------------------------------------------------------------

/// Terminal resize request forwarded to the remote PTY.
#[derive(Debug, Clone, Copy)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

// ---------------------------------------------------------------------------
// ExecSession — an active exec session owned by the TUI
// ---------------------------------------------------------------------------

/// An active pod exec session.
///
/// The session owns a background tokio task that drives the WebSocket
/// connection. The TUI communicates via channels:
/// - `output_rx` — poll for stdout/stderr/exit messages
/// - `stdin_tx` — send raw bytes to the remote process
/// - `resize_tx` — send terminal resize events
pub struct ExecSession {
    /// Whether the session is still active (not exited/closed).
    pub active: bool,
    /// Pod name this session is attached to.
    pub pod_name: String,
    /// Container name (may be empty if the pod has only one container).
    pub container_name: String,
    /// Namespace.
    pub namespace: String,
    /// Accumulated output lines for display in the TUI.
    pub output_lines: Vec<String>,
    /// Current input line being typed by the user.
    pub input_line: String,

    /// Receiver for messages from the background task.
    pub output_rx: mpsc::UnboundedReceiver<ExecMessage>,
    /// Sender for stdin bytes to the background task.
    pub stdin_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Sender for terminal resize events.
    pub resize_tx: mpsc::UnboundedSender<TerminalSize>,

    /// Handle to the background task for cleanup.
    task_handle: Option<JoinHandle<()>>,
}

impl ExecSession {
    // -- Polling / sending -------------------------------------------------

    /// Drain all pending messages from the background task.
    /// Returns the messages received (may be empty).
    /// Automatically marks the session inactive on `Exited` or `Error`.
    pub fn poll_messages(&mut self) -> Vec<ExecMessage> {
        let mut msgs = Vec::new();
        while let Ok(msg) = self.output_rx.try_recv() {
            match &msg {
                ExecMessage::Stdout(data) | ExecMessage::Stderr(data) => {
                    // Convert to lossy UTF-8 lines for the scrollback buffer
                    let text = String::from_utf8_lossy(data);
                    for line in text.split('\n') {
                        if !line.is_empty() {
                            self.output_lines.push(line.to_string());
                        }
                    }
                }
                ExecMessage::Exited(_) | ExecMessage::Error(_) => {
                    self.active = false;
                    // Push a marker line
                    let marker = match &msg {
                        ExecMessage::Exited(Some(reason)) => {
                            format!("[session exited: {}]", reason)
                        }
                        ExecMessage::Exited(None) => "[session exited]".to_string(),
                        ExecMessage::Error(e) => format!("[error: {}]", e),
                        _ => unreachable!(),
                    };
                    self.output_lines.push(marker);
                }
            }
            msgs.push(msg);
        }
        msgs
    }

    /// Send raw bytes to the remote process stdin.
    pub fn send_stdin(&self, data: Vec<u8>) -> Result<(), mpsc::error::SendError<Vec<u8>>> {
        self.stdin_tx.send(data)
    }

    /// Convenience: send a UTF-8 string (e.g. a typed command + newline).
    pub fn send_stdin_str(&self, s: &str) -> Result<(), mpsc::error::SendError<Vec<u8>>> {
        self.send_stdin(s.as_bytes().to_vec())
    }

    /// Notify the remote PTY of a terminal resize.
    pub fn send_resize(
        &self,
        size: TerminalSize,
    ) -> Result<(), mpsc::error::SendError<TerminalSize>> {
        self.resize_tx.send(size)
    }

    /// Close the session, aborting the background task.
    pub fn close(&mut self) {
        self.active = false;
        if let Some(handle) = self.task_handle.take() {
            handle.abort();
        }
    }

    // -- Accessors ---------------------------------------------------------

    /// Title string suitable for rendering in a TUI panel header.
    pub fn title(&self) -> String {
        if self.container_name.is_empty() {
            format!("exec: {}/{}", self.namespace, self.pod_name)
        } else {
            format!(
                "exec: {}/{} ({})",
                self.namespace, self.pod_name, self.container_name
            )
        }
    }

    /// Number of output lines accumulated so far.
    pub fn line_count(&self) -> usize {
        self.output_lines.len()
    }
}

impl Drop for ExecSession {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// Shell candidates tried in order
// ---------------------------------------------------------------------------

const SHELL_CANDIDATES: &[&str] = &["/bin/bash", "/bin/sh"];

// ---------------------------------------------------------------------------
// start_exec_session — spawn the background task
// ---------------------------------------------------------------------------

/// Start a new exec session against a pod.
///
/// This function spawns a tokio task that:
/// 1. Connects to the pod via `kube-rs` exec API over WebSocket.
/// 2. If no explicit command is given, tries shell candidates in order.
/// 3. Bridges stdin/stdout/stderr through channels.
///
/// The returned `ExecSession` is owned by the caller (the TUI event loop).
pub async fn start_exec_session(
    client: kube::Client,
    req: ExecRequest,
) -> Result<ExecSession, kube::Error> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), &req.namespace);

    // Resolve the command list — for shell requests, try candidates
    let commands: Vec<Vec<String>> = if req.command.is_empty() {
        SHELL_CANDIDATES
            .iter()
            .map(|s| vec![s.to_string()])
            .collect()
    } else {
        vec![req.command.clone()]
    };

    // Channels
    let (output_tx, output_rx) = mpsc::unbounded_channel::<ExecMessage>();
    let (stdin_tx, stdin_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (resize_tx, resize_rx) = mpsc::unbounded_channel::<TerminalSize>();

    let container_name = req.container.clone().unwrap_or_default();
    let pod_name = req.pod_name.clone();
    let namespace = req.namespace.clone();
    let tty = req.tty;
    let container = req.container.clone();

    let task_handle = tokio::spawn(exec_task(
        pods,
        pod_name.clone(),
        container.clone(),
        commands,
        tty,
        output_tx,
        stdin_rx,
        resize_rx,
    ));

    Ok(ExecSession {
        active: true,
        pod_name,
        container_name,
        namespace,
        output_lines: Vec::new(),
        input_line: String::new(),
        output_rx,
        stdin_tx,
        resize_tx,
        task_handle: Some(task_handle),
    })
}

/// Background task that drives the exec WebSocket connection.
async fn exec_task(
    pods: Api<Pod>,
    pod_name: String,
    container: Option<String>,
    commands: Vec<Vec<String>>,
    tty: bool,
    output_tx: mpsc::UnboundedSender<ExecMessage>,
    mut stdin_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    mut resize_rx: mpsc::UnboundedReceiver<TerminalSize>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Try each command candidate until one succeeds
    let mut last_err: Option<kube::Error> = None;
    for cmd in &commands {
        let ap = AttachParams {
            stdin: true,
            stdout: true,
            stderr: !tty, // When using a TTY, stderr is muxed into stdout
            tty,
            container: container.clone(),
            ..Default::default()
        };

        let result = pods.exec(&pod_name, cmd, &ap).await;
        match result {
            Ok(mut attached) => {
                // Successfully attached — drive I/O
                let tx = output_tx.clone();

                // Obtain the resize sender before splitting stdout/stderr/stdin,
                // because terminal_size() takes &mut self and we need to call it
                // before moving parts of `attached` into sub-tasks.
                let resize_sender = if tty { attached.terminal_size() } else { None };

                // Take stdout/stderr/stdin handles from the attached process.
                // Each is Option<impl AsyncRead/AsyncWrite>.
                let maybe_stdout = attached.stdout();
                let maybe_stderr = attached.stderr();
                let maybe_stdin = attached.stdin();

                // Stdout reader task
                let tx_stdout = tx.clone();
                let stdout_task = tokio::spawn(async move {
                    if let Some(mut stdout) = maybe_stdout {
                        let mut buf = [0u8; 4096];
                        loop {
                            match stdout.read(&mut buf).await {
                                Ok(0) => break, // EOF
                                Ok(n) => {
                                    let _ = tx_stdout.send(ExecMessage::Stdout(buf[..n].to_vec()));
                                }
                                Err(e) => {
                                    let _ = tx_stdout
                                        .send(ExecMessage::Error(format!("stdout read: {e}")));
                                    break;
                                }
                            }
                        }
                    }
                });

                // Stderr reader task (only active when not TTY)
                let tx_stderr = tx.clone();
                let stderr_task = tokio::spawn(async move {
                    if let Some(mut stderr) = maybe_stderr {
                        let mut buf = [0u8; 4096];
                        loop {
                            match stderr.read(&mut buf).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    let _ = tx_stderr.send(ExecMessage::Stderr(buf[..n].to_vec()));
                                }
                                Err(e) => {
                                    let _ = tx_stderr
                                        .send(ExecMessage::Error(format!("stderr read: {e}")));
                                    break;
                                }
                            }
                        }
                    }
                });

                // Stdin writer + resize forwarder
                let tx_stdin_err = tx.clone();
                let stdin_task = tokio::spawn(async move {
                    use futures::SinkExt;

                    let mut resize_sender = resize_sender;
                    if let Some(mut stdin) = maybe_stdin {
                        loop {
                            tokio::select! {
                                data = stdin_rx.recv() => {
                                    match data {
                                        Some(bytes) => {
                                            if stdin.write_all(&bytes).await.is_err() {
                                                break;
                                            }
                                            if stdin.flush().await.is_err() {
                                                break;
                                            }
                                        }
                                        None => break, // channel closed
                                    }
                                }
                                size = resize_rx.recv() => {
                                    match size {
                                        Some(ts) => {
                                            if let Some(ref mut rtx) = resize_sender {
                                                let kube_size = kube::api::TerminalSize {
                                                    width: ts.cols,
                                                    height: ts.rows,
                                                };
                                                if let Err(e) = rtx.send(kube_size).await {
                                                    let _ = tx_stdin_err.send(ExecMessage::Error(
                                                        format!("resize send: {e}"),
                                                    ));
                                                }
                                            }
                                        }
                                        None => {} // resize channel closed, ignore
                                    }
                                }
                            }
                        }
                    }
                });

                // Wait for stdout/stderr to close (process exited)
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                stdin_task.abort();

                // Join the attached process to collect the exit status
                let status = attached.join().await;
                let exit_reason = match status {
                    Ok(()) => None,
                    Err(e) => Some(format!("{e}")),
                };

                let _ = tx.send(ExecMessage::Exited(exit_reason));
                return;
            }
            Err(e) => {
                last_err = Some(e);
                // Try next candidate
                continue;
            }
        }
    }

    // All candidates failed
    let err_msg = last_err
        .map(|e| format!("exec failed: {e}"))
        .unwrap_or_else(|| "exec failed: no commands to try".to_string());
    let _ = output_tx.send(ExecMessage::Error(err_msg));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ExecRequest constructors ------------------------------------------

    #[test]
    fn exec_request_shell_defaults() {
        let req = ExecRequest::shell("my-pod", "default");
        assert_eq!(req.pod_name, "my-pod");
        assert_eq!(req.namespace, "default");
        assert!(req.command.is_empty(), "shell request has no fixed command");
        assert!(req.tty, "shell request defaults to tty=true");
        assert!(req.container.is_none());
    }

    #[test]
    fn exec_request_with_command() {
        let req =
            ExecRequest::with_command("pod-1", "kube-system", vec!["ls".into(), "-la".into()]);
        assert_eq!(req.pod_name, "pod-1");
        assert_eq!(req.namespace, "kube-system");
        assert_eq!(req.command, vec!["ls", "-la"]);
        assert!(!req.tty, "with_command defaults to tty=false");
    }

    #[test]
    fn exec_request_builder_chain() {
        let req = ExecRequest::shell("pod", "ns")
            .container("sidecar")
            .tty(false);
        assert_eq!(req.container.as_deref(), Some("sidecar"));
        assert!(!req.tty);
    }

    // -- ExecMessage variants ----------------------------------------------

    #[test]
    fn exec_message_debug() {
        // Ensure all variants are Debug-printable (compile-time + runtime check)
        let msgs = vec![
            ExecMessage::Stdout(b"hello".to_vec()),
            ExecMessage::Stderr(b"warn".to_vec()),
            ExecMessage::Exited(None),
            ExecMessage::Exited(Some("signal 9".into())),
            ExecMessage::Error("connection lost".into()),
        ];
        for m in &msgs {
            let _ = format!("{:?}", m);
        }
    }

    #[test]
    fn exec_message_clone() {
        let msg = ExecMessage::Stdout(vec![1, 2, 3]);
        let cloned = msg.clone();
        match cloned {
            ExecMessage::Stdout(data) => assert_eq!(data, vec![1, 2, 3]),
            _ => panic!("unexpected variant"),
        }
    }

    // -- TerminalSize ------------------------------------------------------

    #[test]
    fn terminal_size_copy() {
        let ts = TerminalSize { cols: 80, rows: 24 };
        let ts2 = ts; // Copy
        assert_eq!(ts2.cols, 80);
        assert_eq!(ts2.rows, 24);
    }

    // -- ExecSession unit tests (no cluster needed) ------------------------

    /// Helper: create an ExecSession with wired-up channels for testing.
    fn make_test_session() -> (
        ExecSession,
        mpsc::UnboundedSender<ExecMessage>,
        mpsc::UnboundedReceiver<Vec<u8>>,
        mpsc::UnboundedReceiver<TerminalSize>,
    ) {
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();
        let (resize_tx, resize_rx) = mpsc::unbounded_channel();

        let session = ExecSession {
            active: true,
            pod_name: "test-pod".into(),
            container_name: "app".into(),
            namespace: "default".into(),
            output_lines: Vec::new(),
            input_line: String::new(),
            output_rx,
            stdin_tx,
            resize_tx,
            task_handle: None,
        };

        (session, output_tx, stdin_rx, resize_rx)
    }

    #[test]
    fn session_title_with_container() {
        let (session, _, _, _) = make_test_session();
        assert_eq!(session.title(), "exec: default/test-pod (app)");
    }

    #[test]
    fn session_title_without_container() {
        let (mut session, _, _, _) = make_test_session();
        session.container_name = String::new();
        assert_eq!(session.title(), "exec: default/test-pod");
    }

    #[test]
    fn session_line_count_starts_at_zero() {
        let (session, _, _, _) = make_test_session();
        assert_eq!(session.line_count(), 0);
    }

    #[test]
    fn poll_messages_stdout() {
        let (mut session, tx, _, _) = make_test_session();
        tx.send(ExecMessage::Stdout(b"hello world\n".to_vec()))
            .unwrap();
        tx.send(ExecMessage::Stdout(b"line two\n".to_vec()))
            .unwrap();

        let msgs = session.poll_messages();
        assert_eq!(msgs.len(), 2);
        assert!(session.active);
        assert_eq!(session.line_count(), 2);
        assert_eq!(session.output_lines[0], "hello world");
        assert_eq!(session.output_lines[1], "line two");
    }

    #[test]
    fn poll_messages_stderr() {
        let (mut session, tx, _, _) = make_test_session();
        tx.send(ExecMessage::Stderr(b"error output\n".to_vec()))
            .unwrap();

        let msgs = session.poll_messages();
        assert_eq!(msgs.len(), 1);
        assert!(session.active);
        assert_eq!(session.output_lines[0], "error output");
    }

    #[test]
    fn poll_messages_exited_marks_inactive() {
        let (mut session, tx, _, _) = make_test_session();
        tx.send(ExecMessage::Exited(None)).unwrap();

        let msgs = session.poll_messages();
        assert_eq!(msgs.len(), 1);
        assert!(!session.active);
        assert_eq!(session.output_lines[0], "[session exited]");
    }

    #[test]
    fn poll_messages_exited_with_reason() {
        let (mut session, tx, _, _) = make_test_session();
        tx.send(ExecMessage::Exited(Some("signal 15".into())))
            .unwrap();

        session.poll_messages();
        assert!(!session.active);
        assert_eq!(session.output_lines[0], "[session exited: signal 15]");
    }

    #[test]
    fn poll_messages_error_marks_inactive() {
        let (mut session, tx, _, _) = make_test_session();
        tx.send(ExecMessage::Error("connection refused".into()))
            .unwrap();

        session.poll_messages();
        assert!(!session.active);
        assert_eq!(session.output_lines[0], "[error: connection refused]");
    }

    #[test]
    fn poll_messages_empty_when_no_messages() {
        let (mut session, _tx, _, _) = make_test_session();
        let msgs = session.poll_messages();
        assert!(msgs.is_empty());
        assert!(session.active);
    }

    #[test]
    fn send_stdin_bytes() {
        let (session, _, mut stdin_rx, _) = make_test_session();
        session.send_stdin(b"ls\n".to_vec()).unwrap();

        let received = stdin_rx.try_recv().unwrap();
        assert_eq!(received, b"ls\n");
    }

    #[test]
    fn send_stdin_str() {
        let (session, _, mut stdin_rx, _) = make_test_session();
        session.send_stdin_str("echo hello\n").unwrap();

        let received = stdin_rx.try_recv().unwrap();
        assert_eq!(received, b"echo hello\n");
    }

    #[test]
    fn send_resize() {
        let (session, _, _, mut resize_rx) = make_test_session();
        session
            .send_resize(TerminalSize {
                cols: 120,
                rows: 40,
            })
            .unwrap();

        let received = resize_rx.try_recv().unwrap();
        assert_eq!(received.cols, 120);
        assert_eq!(received.rows, 40);
    }

    #[test]
    fn close_marks_inactive() {
        let (mut session, _, _, _) = make_test_session();
        assert!(session.active);
        session.close();
        assert!(!session.active);
    }

    #[test]
    fn close_is_idempotent() {
        let (mut session, _, _, _) = make_test_session();
        session.close();
        session.close(); // should not panic
        assert!(!session.active);
    }

    #[test]
    fn poll_multiple_mixed_messages() {
        let (mut session, tx, _, _) = make_test_session();
        tx.send(ExecMessage::Stdout(b"line1\nline2\n".to_vec()))
            .unwrap();
        tx.send(ExecMessage::Stderr(b"warning\n".to_vec())).unwrap();
        tx.send(ExecMessage::Stdout(b"line3\n".to_vec())).unwrap();

        let msgs = session.poll_messages();
        assert_eq!(msgs.len(), 3);
        assert!(session.active);
        // line1, line2 from first msg, warning from second, line3 from third
        assert_eq!(session.line_count(), 4);
    }

    #[test]
    fn shell_candidates_are_valid() {
        assert!(SHELL_CANDIDATES.len() >= 2);
        assert!(SHELL_CANDIDATES.contains(&"/bin/bash"));
        assert!(SHELL_CANDIDATES.contains(&"/bin/sh"));
    }

    #[test]
    fn output_lines_accumulate_across_polls() {
        let (mut session, tx, _, _) = make_test_session();
        tx.send(ExecMessage::Stdout(b"first\n".to_vec())).unwrap();
        session.poll_messages();
        assert_eq!(session.line_count(), 1);

        tx.send(ExecMessage::Stdout(b"second\n".to_vec())).unwrap();
        session.poll_messages();
        assert_eq!(session.line_count(), 2);
        assert_eq!(session.output_lines[0], "first");
        assert_eq!(session.output_lines[1], "second");
    }

    #[test]
    fn binary_output_handled_as_lossy_utf8() {
        let (mut session, tx, _, _) = make_test_session();
        // Invalid UTF-8 bytes
        tx.send(ExecMessage::Stdout(vec![0xFF, 0xFE, 0x68, 0x69]))
            .unwrap();
        session.poll_messages();
        // Should not panic; lossy conversion replaces invalid bytes
        assert_eq!(session.line_count(), 1);
        assert!(session.output_lines[0].contains("hi"));
    }
}
