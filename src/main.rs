//! mcp-server-bridge — main.rs
//! CLI entry point, shared state, axum router.

mod handlers;
mod process;
mod session;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};
use clap::Parser;
use tokio::net::lookup_host;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt};

use process::ProcessConfig;
use session::SessionStore;

/// Shared application state threaded through axum.
#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionStore,
    pub process_cfg: Arc<ProcessConfig>,
}

/// Try to replace a hostname in a URL string with its resolved IP address.
/// This is required for Chrome's DevTools Protocol HTTP server, which rejects
/// requests whose `Host` header is a hostname (returns HTTP 500).
/// Example: "http://host.docker.internal:9322" → "http://192.168.65.254:9322"
async fn resolve_hostname_in_url(url: &str) -> String {
    // Parse the URL well enough to extract host:port
    let after_scheme = if let Some(s) = url.strip_prefix("http://") {
        ("http", s)
    } else if let Some(s) = url.strip_prefix("https://") {
        ("https", s)
    } else {
        return url.to_string(); // Can't parse, return as-is
    };

    let (scheme, rest) = after_scheme;
    // rest = "host.docker.internal:9322/some/path"
    let (hostport, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };

    // Skip resolution if already an IP address (contains only digits and dots/colons)
    let host_only = hostport.split(':').next().unwrap_or(hostport);
    if host_only.parse::<std::net::IpAddr>().is_ok() {
        return url.to_string();
    }

    // Ensure hostport has a port for lookup_host
    let lookup_target = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{}:80", hostport)
    };

    let lookup_result = lookup_host(lookup_target.as_str()).await;
    match lookup_result {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.next() {
                let ip = addr.ip();
                let port_suffix = if hostport.contains(':') {
                    format!(":{}", hostport.split(':').nth(1).unwrap_or("80"))
                } else {
                    String::new()
                };
                let resolved = format!("{}://{}{}{}", scheme, ip, port_suffix, path);
                info!(original = %url, resolved = %resolved, "Resolved hostname to IP for Chrome DevTools compat");
                resolved
            } else {
                warn!(url = %url, "DNS lookup returned no addresses, keeping original");
                url.to_string()
            }
        }
        Err(e) => {
            warn!(url = %url, error = %e, "DNS lookup failed, keeping original hostname");
            url.to_string()
        }
    }
}

/// MCP Server Bridge — expose a stdio MCP server over HTTP/SSE.
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Args {
    /// TCP port to listen on.
    #[arg(long, env = "PORT", default_value = "3000")]
    pub port: u16,

    /// Bind address.
    #[arg(long, env = "BIND_ADDR", default_value = "0.0.0.0")]
    pub bind: String,

    /// Docker container name to exec into.
    #[arg(long, env = "MCP_CONTAINER", default_value = "chrome-mcp-server")]
    pub container: String,

    /// MCP executable inside the container.
    #[arg(long, env = "MCP_COMMAND", default_value = "chrome-devtools-mcp")]
    pub mcp_command: String,

    /// Extra arguments forwarded to the MCP server (repeat flag or space-separated).
    /// Example: --mcp-arg=--browser-url=http://host.docker.internal:9322
    #[arg(long = "mcp-arg", env = "MCP_ARGS", value_delimiter = ' ')]
    pub mcp_args: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialise tracing from RUST_LOG env (default: info)
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    info!("@@@ MCP BRIDGE VERSION 16 - KILL ORPHAN MCP PROCESSES @@@");

    // Clap Vec env can be tricky. Let's explicitly check the env var.
    let mut mcp_args: Vec<String> = args.mcp_args.into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect();

    if let Ok(val) = std::env::var("MCP_ARGS") {
        info!(env_val = %val, "DEBUG: Found MCP_ARGS in env");
        if mcp_args.is_empty() && !val.trim().is_empty() {
            mcp_args = val.split_whitespace().map(|s| s.to_string()).collect();
        }
    } else {
        info!("DEBUG: MCP_ARGS NOT FOUND in env");
    }

    // --- DNS Pre-Resolution ---
    // Chrome DevTools Protocol HTTP server rejects requests with a hostname in
    // the Host header (returns HTTP 500). Resolve any hostname in --browser-url
    // args to a raw IP address before handing them to chrome-devtools-mcp.
    let mut resolved_args: Vec<String> = Vec::with_capacity(mcp_args.len());
    for arg in &mcp_args {
        // Handle both "--browser-url=http://..." and "--browser-url" "http://..." forms
        if let Some(url) = arg.strip_prefix("--browser-url=") {
            let resolved_url = resolve_hostname_in_url(url).await;
            resolved_args.push(format!("--browser-url={}", resolved_url));
        } else {
            resolved_args.push(arg.clone());
        }
    }
    info!(original_args = ?mcp_args, resolved_args = ?resolved_args, "MCP args after hostname resolution");

    let process_cfg = Arc::new(ProcessConfig {
        container: args.container.clone(),
        mcp_command: args.mcp_command.clone(),
        mcp_args: resolved_args.clone(),
    });

    let state = AppState {
        sessions: session::new_store(),
        process_cfg,
    };

    let cors = CorsLayer::permissive();

    let app = Router::new()
        .route("/mcp", get(handlers::sse_handler))
        .route("/mcp", post(handlers::post_handler))
        .route("/health", get(handlers::health_handler))
        .layer(cors)
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    info!(%addr, container = %args.container, mcp_command = %args.mcp_command, mcp_args = ?resolved_args, "mcp-server-bridge listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
