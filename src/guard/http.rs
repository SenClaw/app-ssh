//! The guard as the rest of the app sees it: one call that gates a command,
//! and the REST routes the Approvals screen uses.
//!
//! | Route | |
//! |---|---|
//! | `GET /api/guard/approvals` | every request, newest first |
//! | `POST /api/guard/approvals/:id/approve` | `{remember}` — runs it on its target |
//! | `POST /api/guard/approvals/:id/reject` | `{note}` |
//! | `GET /api/guard/learned` / `DELETE /api/guard/learned` | the learned list / `{command}` |
//! | `POST /api/guard/check` | `{command}` — what would happen, nothing learned or run |

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use super::approvals::Approval;
use super::learned::LearnedCommand;
use super::{Action, Decision};
use crate::api::AppState;
use crate::logs;
use crate::remote::{self, Target};

/// How long an approved command may run before it is reported as failed.
const APPROVED_RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

pub enum Gate {
    Run,
    Wait(Approval),
    Refused(Decision),
}

/// Decide what happens to `command`; park it for the admin when it must wait.
pub async fn gate(state: &Arc<AppState>, command: &str, target: Target, target_label: &str, origin: &str) -> Gate {
    let settings = state.settings.get();
    let decision = state.guard.decide(&settings, command, true).await;
    let host = Some(target_label.to_string());
    match decision.action {
        Action::Run => {
            let how = decision
                .checks
                .iter()
                .find(|c| c.outcome == "pass")
                .map(|c| c.detail.clone())
                .unwrap_or_default();
            let learned = if decision.learned { " (learned)" } else { "" };
            logs::info(&state.log_store, "guard", "allow", host, format!("Allowed '{command}': {how}{learned}"));
            Gate::Run
        }
        Action::Block => {
            logs::warn(&state.log_store, "guard", "block", host, format!("Blocked '{command}': {}", decision.why()));
            Gate::Refused(decision)
        }
        Action::Ask => {
            let a = state
                .guard
                .approvals
                .create(command, target_label, target, origin, decision.checks.clone());
            logs::warn(
                &state.log_store,
                "guard",
                "ask",
                host,
                format!("Waiting for the admin: '{command}' — {}", decision.why()),
            );
            announce(state, &a);
            Gate::Wait(a)
        }
    }
}

/// Tell the UI a request appeared or changed.
pub fn announce(state: &AppState, a: &Approval) {
    let _ = state.ui_tx.send(
        json!({
            "type": "guard_approval",
            "approval": a,
            "pending": state.guard.approvals.pending_count(),
        })
        .to_string(),
    );
}

/// What the agent reads when its command has to wait.
pub fn pending_text(a: &Approval) -> String {
    let why = super::Decision {
        action: Action::Ask,
        checks: a.checks.clone(),
        learned: false,
    }
    .why();
    format!(
        "NOT RUN YET: this command needs the admin's approval.\n\
         approval_id: {id}\n\
         command: {cmd}\n\
         why: {why}\n\n\
         Tell the user to approve or reject it in SSH Manager → Approvals. Once approved, SSH Manager runs it \
         itself on the same target; read the result with ssh_command_status {{\"approval_id\": \"{id}\"}} \
         (status: pending → running → done | failed | rejected | expired). To wait without polling in your turn, \
         arm a schedule_watch on ssh_command_status until status is one of done, failed, rejected, expired. \
         Do not re-send or reword the command to get around this check.",
        id = a.id,
        cmd = a.command,
    )
}

/// `ssh_command_status`'s answer: JSON whose top-level `status` a watch can test.
pub fn status_json(a: &Approval) -> Value {
    json!({
        "approval_id": a.id,
        "status": a.status,
        "command": a.command,
        "target": a.target,
        "output": a.output,
        "error": a.error,
        "note": a.note,
        "checks": a.checks,
    })
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/approvals", get(list_approvals))
        .route("/approvals/:id/approve", post(approve))
        .route("/approvals/:id/reject", post(reject))
        .route("/learned", get(list_learned).delete(remove_learned))
        .route("/check", post(dry_run))
}

type Refusal = (StatusCode, Json<Value>);

fn refuse(status: StatusCode, message: impl Into<String>) -> Refusal {
    (status, Json(json!({ "error": message.into() })))
}

async fn list_approvals(State(state): State<Arc<AppState>>) -> Json<Vec<Approval>> {
    Json(state.guard.approvals.list())
}

#[derive(Deserialize, Default)]
struct ApproveBody {
    #[serde(default)]
    remember: bool,
}

async fn approve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<ApproveBody>>,
) -> Result<Json<Approval>, Refusal> {
    let remember = body.map(|b| b.0.remember).unwrap_or(false);
    let a = state
        .guard
        .approvals
        .approve(&id, remember)
        .map_err(|e| refuse(StatusCode::CONFLICT, e))?;
    if remember {
        state.guard.learned.add(&a.command, "admin", "approved by the admin");
    }
    let remembered = if remember { " and remembered it" } else { "" };
    logs::info(
        &state.log_store,
        "guard",
        "approve",
        Some(a.target.clone()),
        format!("Admin approved '{}'{remembered}", a.command),
    );
    announce(&state, &a);

    let run_state = state.clone();
    let job = a.clone();
    tokio::spawn(async move {
        // A command that hangs must not stay "running" for ever.
        let result = match tokio::time::timeout(
            APPROVED_RUN_TIMEOUT,
            remote::run(&run_state, &job.run_on, &job.command),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(format!(
                "no answer from the server after {} minutes; the command may still be running there",
                APPROVED_RUN_TIMEOUT.as_secs() / 60
            )),
        };
        if let Some(done) = run_state.guard.approvals.finish(&job.id, result) {
            match &done.error {
                None => logs::info(
                    &run_state.log_store,
                    "guard",
                    "approved_run",
                    Some(done.target.clone()),
                    format!("Ran approved '{}'", done.command),
                ),
                Some(e) => logs::error(
                    &run_state.log_store,
                    "guard",
                    "approved_run",
                    Some(done.target.clone()),
                    format!("Approved '{}' could not run: {e}", done.command),
                ),
            }
            announce(&run_state, &done);
        }
    });
    Ok(Json(a))
}

#[derive(Deserialize, Default)]
struct RejectBody {
    note: Option<String>,
}

async fn reject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<RejectBody>>,
) -> Result<Json<Approval>, Refusal> {
    let note = body.and_then(|b| b.0.note);
    let a = state
        .guard
        .approvals
        .reject(&id, note)
        .map_err(|e| refuse(StatusCode::CONFLICT, e))?;
    logs::info(
        &state.log_store,
        "guard",
        "reject",
        Some(a.target.clone()),
        format!("Admin rejected '{}'", a.command),
    );
    announce(&state, &a);
    Ok(Json(a))
}

async fn list_learned(State(state): State<Arc<AppState>>) -> Json<Vec<LearnedCommand>> {
    Json(state.guard.learned.list())
}

#[derive(Deserialize)]
struct CommandBody {
    command: String,
}

async fn remove_learned(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CommandBody>,
) -> Result<Json<Value>, Refusal> {
    if state.guard.learned.remove(&body.command) {
        logs::info(
            &state.log_store,
            "guard",
            "forget",
            None,
            format!("Removed '{}' from the learned commands", body.command),
        );
        Ok(Json(json!({ "removed": true })))
    } else {
        Err(refuse(StatusCode::NOT_FOUND, "that command is not in the learned list"))
    }
}

async fn dry_run(State(state): State<Arc<AppState>>, Json(body): Json<CommandBody>) -> Json<Decision> {
    let settings = state.settings.get();
    Json(state.guard.decide(&settings, &body.command, false).await)
}
