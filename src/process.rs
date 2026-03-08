//! mcp-server-bridge — process.rs
//! Spawns a stdio MCP child process (docker exec -i ...) and returns async handles.

use std::process::Stdio;
use tokio::process::{Child, Command};

/// Configuration for the MCP child process.
#[derive(Debug, Clone)]
pub struct ProcessConfig {
    /// Docker container name to exec into (e.g. "chrome-mcp-server")
    pub container: String,
    /// Command to run inside the container (e.g. "chrome-devtools-mcp")
    pub mcp_command: String,
    /// Extra args forwarded to the MCP server (e.g. ["--browser-url=http://..."])
    pub mcp_args: Vec<String>,
}

/// Spawn the MCP child process.
/// Returns a `Child` handle with stdin/stdout pipes attached.
pub fn spawn_mcp_process(cfg: &ProcessConfig) -> std::io::Result<Child> {
    let mut cmd = Command::new("docker");
    cmd.arg("exec")
        .arg("-i")
        .arg(&cfg.container)
        .arg(&cfg.mcp_command);

    for arg in cfg.mcp_args.iter().filter(|s| !s.trim().is_empty()) {
        cmd.arg(arg);
    }

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit()) // surface MCP stderr to bridge logs
        .kill_on_drop(true)
        .spawn()
}
