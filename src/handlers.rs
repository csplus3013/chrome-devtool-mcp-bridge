//! mcp-server-bridge — handlers.rs
//! Axum route handlers: SSE (GET /mcp), POST /mcp, GET /health.

use std::convert::Infallible;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{sse::Event, IntoResponse, Sse},
};
use futures_util::stream::{self, StreamExt};
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    session::{create_session, remove_session},
    AppState,
};

/// GET /mcp — open an SSE stream, spawn a child MCP process for this session.
pub async fn sse_handler(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let cfg = state.process_cfg.clone();
    let store = state.sessions.clone();

    let session_id = match create_session(&store, &cfg).await {
        Ok(id) => id,
        Err(e) => {
            error!(?e, "could not create session");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to start MCP process")
                .into_response();
        }
    };

    info!(session_id = %session_id, "SSE client connected");

    let rx = {
        let entry = store.get(&session_id);
        match entry {
            Some(ref s) => s.sse_tx.subscribe(),
            None => {
                error!("session vanished immediately after creation");
                return (StatusCode::INTERNAL_SERVER_ERROR, "session not found").into_response();
            }
        }
    };

    let broadcast_stream = BroadcastStream::new(rx);
    let store_clone = store.clone();
    let sid = session_id;

    // Determine the absolute base URL for the 'endpoint' event.
    // Standard MCP clients work best if this is an absolute URL or a path they can resolve.
    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("127.0.0.1:3000");
    // We assume http for now as this is a local bridge.
    let absolute_endpoint = format!("http://{}/mcp?session_id={}", host, sid);

    let event_stream = broadcast_stream.filter_map(move |result| {
        let val = match result {
            Ok(line) => Some(Ok::<Event, Infallible>(
                Event::default().event("message").data(line),
            )),
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                warn!(lagged = n, "SSE consumer lagged behind");
                None
            }
        };
        async move { val }
    });

    let endpoint_event = stream::once(async move {
        Ok::<Event, Infallible>(
            Event::default()
                .event("endpoint")
                .data(absolute_endpoint),
        )
    });

    let full_stream = endpoint_event.chain(event_stream).chain(stream::once(async move {
        remove_session(&store_clone, &sid);
        Ok::<Event, Infallible>(Event::default().comment("bye"))
    }));

    let sse = Sse::new(full_stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    );

    let mut response = sse.into_response();
    response.headers_mut().insert("x-session-id", sid.to_string().parse().unwrap());
    response
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct SessionQuery {
    #[serde(alias = "sessionId", alias = "sid", alias = "session")]
    pub session_id: Option<String>,
}

/// POST /mcp — receive a JSON-RPC message and forward it to the session's child stdin.
pub async fn post_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: axum::extract::RawQuery,
    body: String,
) -> impl IntoResponse {
    // Manually parse the query to avoid 400 errors if it is malformed or missing
    let query_params: SessionQuery = query.0.clone().and_then(|q| serde_urlencoded::from_str(&q).ok())
        .unwrap_or(SessionQuery { session_id: None });

    let sid_from_query = query_params.session_id;

    let sid_from_header = headers.get("x-session-id")
        .or_else(|| headers.get("session_id"))
        .or_else(|| headers.get("sessionId"))
        .or_else(|| headers.get("session-id"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let session_id_str = sid_from_query.or(sid_from_header);

    let session_id = match session_id_str.as_deref().and_then(|s| Uuid::parse_str(s).ok()) {
        Some(id) => id,
        None => {
            warn!(
                raw_query = ?query.0,
                headers = ?headers,
                "POST rejected: missing or invalid session_id"
            );
            return (
                StatusCode::BAD_REQUEST,
                format!("missing or invalid session_id. Received raw_query={:?}", query.0),
            ).into_response();
        }
    };

    let sent = match state.sessions.get(&session_id) {
        Some(session) => session.stdin_tx.send(body).await.is_ok(),
        None => false,
    };

    if sent {
        StatusCode::ACCEPTED.into_response()
    } else {
        warn!(%session_id, "session not found for POST - maybe it was closed?");
        // Return 410 Gone so clients know to re-establish the SSE stream.
        // 404 can be ambiguous (wrong path?); 410 clearly means "this session
        // existed but is permanently gone — open a new SSE connection".
        let mut resp = (
            StatusCode::GONE,
            "session expired or not found — please reconnect SSE to get a new session_id",
        ).into_response();
        resp.headers_mut().insert(
            "x-mcp-reconnect",
            "true".parse().unwrap(),
        );
        resp
    }
}

/// GET /health — liveness probe.
pub async fn health_handler() -> impl IntoResponse {
    axum::Json(json!({"status": "ok"}))
}
