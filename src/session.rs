//! mcp-server-bridge — session.rs
//! Per-connection session: owns the child process, channels message to/from it.

use std::sync::Arc;

use dashmap::DashMap;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Child,
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::process::{spawn_mcp_process, ProcessConfig};

/// One message flowing from the bridge → SSE client.
pub type SseMessage = String;

/// Handle for a single MCP session.
///
/// Dropping this struct will:
/// - cancel the CancellationToken → stops reader/writer tasks cleanly
/// - abort the reader/writer JoinHandles (Tokio tasks are detached on drop
///   unless explicitly aborted, so we abort them here)
/// - drop the Child → kill_on_drop fires, sending SIGKILL to `docker exec`
///   which in turn terminates the chrome-devtools-mcp process inside the container
pub struct Session {
    /// Send a JSON-RPC line to the child's stdin.
    pub stdin_tx: mpsc::Sender<String>,
    /// Subscribe to JSON-RPC lines coming from child's stdout.
    pub sse_tx: broadcast::Sender<SseMessage>,
    /// Cancel token — signalled when the session is torn down.
    pub cancel: CancellationToken,
    // The child MUST be stored here so kill_on_drop fires when Session is dropped.
    _child: Child,
    _reader_task: JoinHandle<()>,
    _writer_task: JoinHandle<()>,
    _watcher_task: JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // Signal cancellation first (lets tasks exit their select loops cleanly).
        self.cancel.cancel();
        // Abort detached tasks so they don't linger after the Session is gone.
        self._reader_task.abort();
        self._writer_task.abort();
        self._watcher_task.abort();
        // _child is dropped here → kill_on_drop → SIGKILL to docker exec process
    }
}

/// Thread-safe map of session_id → Session.
pub type SessionStore = Arc<DashMap<Uuid, Session>>;

/// Create a new `SessionStore`.
pub fn new_store() -> SessionStore {
    Arc::new(DashMap::new())
}

/// Spawn a new session: starts the child process and two bridge tasks.
///
/// Returns the session ID so the caller can subscribe via `sse_tx`.
pub async fn create_session(
    store: &SessionStore,
    cfg: &ProcessConfig,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let id = Uuid::new_v4();
    let cancel = CancellationToken::new();

    // Spawn child process (kill_on_drop = true is set in process.rs)
    let mut child = spawn_mcp_process(cfg).map_err(|e| {
        error!(?e, "failed to spawn MCP child process");
        e
    })?;

    let child_stdin = child.stdin.take().expect("stdin piped");
    let child_stdout = child.stdout.take().expect("stdout piped");

    // Channel: bridge handler → child stdin
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);

    // Broadcast: child stdout → all SSE subscribers (typically 1)
    let (sse_tx, _) = broadcast::channel::<SseMessage>(256);
    let sse_tx_clone = sse_tx.clone();

    let cancel_writer = cancel.clone();
    let cancel_reader = cancel.clone();

    // Writer task: forward stdin_rx → child stdin
    let writer_task = tokio::spawn(async move {
        let mut child_stdin = child_stdin;
        loop {
            tokio::select! {
                _ = cancel_writer.cancelled() => {
                    debug!("writer task cancelled");
                    break;
                }
                msg = stdin_rx.recv() => {
                    match msg {
                        Some(line) => {
                            debug!(line = %line, "→ child stdin");
                            let with_newline = if line.ends_with('\n') {
                                line
                            } else {
                                format!("{}\n", line)
                            };
                            if let Err(e) = child_stdin.write_all(with_newline.as_bytes()).await {
                                error!(?e, "write to child stdin failed");
                                break;
                            }
                            if let Err(e) = child_stdin.flush().await {
                                error!(?e, "flush child stdin failed");
                                break;
                            }
                        }
                        None => {
                            debug!("stdin channel closed");
                            break;
                        }
                    }
                }
            }
        }
    });

    // Reader task: child stdout → sse_tx broadcast
    let reader_task = tokio::spawn(async move {
        let reader = BufReader::new(child_stdout);
        let mut lines = reader.lines();
        loop {
            tokio::select! {
                _ = cancel_reader.cancelled() => {
                    debug!("reader task cancelled");
                    break;
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(l)) if !l.is_empty() => {
                            debug!(line = %l, "← child stdout");
                            // Ignore send errors (no subscribers yet OK)
                            let _ = sse_tx_clone.send(l);
                        }
                        Ok(Some(_)) => {} // empty line, skip
                        Ok(None) => {
                            info!("child stdout EOF");
                            break;
                        }
                        Err(e) => {
                            error!(?e, "read from child stdout failed");
                            break;
                        }
                    }
                }
            }
        }
    });

    // Watcher task: if the child exits on its own, clean up the session entry.
    // NOTE: We no longer hold `child` here — we store it in the Session struct.
    //       Instead we watch the cancel token being triggered by Session::drop,
    //       or by the reader task detecting stdout EOF (which signals process exit).
    //       The watcher simply removes the store entry when cancel fires.
    let cancel_watcher = cancel.clone();
    let id_copy = id;
    let store_clone = store.clone();
    let watcher_task = tokio::spawn(async move {
        cancel_watcher.cancelled().await;
        // Session may already have been removed by remove_session(), that is fine.
        if store_clone.remove(&id_copy).is_some() {
            warn!(session_id = %id_copy, "session cleaned up by watcher (process exit or cancel)");
        }
    });

    let session = Session {
        stdin_tx,
        sse_tx,
        cancel,
        _child: child,
        _reader_task: reader_task,
        _writer_task: writer_task,
        _watcher_task: watcher_task,
    };

    store.insert(id, session);
    info!(session_id = %id, "session created");
    Ok(id)
}

/// Remove a session by ID.
/// The Session's Drop impl will cancel the token, abort tasks, and kill the child.
pub fn remove_session(store: &SessionStore, id: &Uuid) {
    if store.remove(id).is_some() {
        // Session drop fires here → cancel + abort + kill_on_drop
        info!(session_id = %id, "session removed");
    }
}
