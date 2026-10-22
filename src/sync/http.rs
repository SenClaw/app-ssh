//! `/api/sync/*`: the app's side of the sync contract, called by the SenClaw
//! daemon (never by a browser or another program).
//!
//! The router is mounted behind `app_space_sdk::auth::require_app_token`, so
//! under SenClaw only a request carrying this app's token gets in; the daemon
//! stamps that token on everything it forwards. Without a token in the
//! environment (a bare `cargo run`) the guard lets everything through, which is
//! why the daemon probes these routes without a token before it will use them.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::SyncSource;
use super::engine::{ApplyRequest, Engine, SyncError};
use crate::api::AppState;
use crate::logs;

type Reply = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn engine(state: &AppState) -> Engine<'_> {
    let sources: Vec<&dyn SyncSource> =
        vec![&state.keychain, &state.hosts, &state.port_forwarding_store, &state.snippets];
    Engine::new(&state.sync, sources)
}

fn refuse(e: SyncError) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() })))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/schema", get(schema))
        .route("/state", post(digest))
        .route("/read", post(read))
        .route("/apply", post(apply))
}

async fn schema(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(engine(&state).schema())
}

#[derive(Deserialize, Default)]
struct StateRequest {
    #[serde(default)]
    collections: Option<Vec<String>>,
}

async fn digest(State(state): State<Arc<AppState>>, body: Option<Json<StateRequest>>) -> Reply {
    let only = body.and_then(|Json(b)| b.collections);
    engine(&state).digest(only.as_deref()).map(Json).map_err(refuse)
}

#[derive(Deserialize)]
struct ReadRequest {
    collection: String,
    ids: Vec<String>,
    /// Include secret fields. Off unless the caller has been allowed to move them.
    #[serde(default)]
    secrets: bool,
}

async fn read(State(state): State<Arc<AppState>>, Json(req): Json<ReadRequest>) -> Reply {
    engine(&state)
        .read(&req.collection, &req.ids, req.secrets)
        .map(|records| Json(json!({ "records": records })))
        .map_err(refuse)
}

async fn apply(State(state): State<Arc<AppState>>, Json(req): Json<ApplyRequest>) -> Reply {
    let report = engine(&state).apply(&req).map_err(refuse)?;
    if !req.dry_run {
        // Counts only: the log must not carry hosts, names or secrets.
        logs::info(
            &state.log_store,
            "sync",
            "apply",
            None,
            format!("Applied a sync batch: {}", serde_json::to_string(&report.summary).unwrap_or_default()),
        );
    }
    Ok(Json(json!(report)))
}
