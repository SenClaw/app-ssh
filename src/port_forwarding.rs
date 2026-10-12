//! Local port forwarding (`ssh -L`), with a status that reflects reality.
//!
//! The stored `active` flag is a *desire*: the user asked for this tunnel to be
//! up, and that survives a restart. Whether the tunnel is actually carrying
//! traffic is a runtime fact, and the two used to be the same field — `start`
//! wrote `active = true` before anything had proven the SSH side was reachable,
//! so a rule pointing at a dead host still read "Active" until someone tried to
//! use it, and after a restart every saved rule claimed to be active with no
//! listener bound at all.
//!
//! Status now comes from [`PortForwardingManager`], which knows what is
//! actually running: a rule is `active` only while its listener is bound and
//! its SSH session is alive, `error` when we know why it is not (and can say
//! so), and `inactive` otherwise. The stored flag lives on as `enabled` and is
//! what [`restore_saved_tunnels`] re-arms on boot.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use russh::Channel;
use russh::client::Msg;
use serde::Serialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::api::AppState;
use crate::client::SshClient;
use crate::models::{Host, PortForwardingRule};

/// How often a running tunnel checks that its SSH session is still up. Short
/// enough that a dropped session shows as an error rather than as a tunnel
/// that silently swallows connections; long enough to be free.
const HEALTH_EVERY: Duration = Duration::from_secs(15);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// What a rule is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelState {
    /// No listener bound; nothing known to be wrong.
    Inactive,
    /// Listener bound and the SSH session behind it is healthy.
    Active,
    /// We know why this rule is not forwarding — see `last_error`.
    Error,
}

/// Live counters for one running tunnel. Shared between the accept loop, every
/// piped connection and whoever asks for status.
pub struct TunnelStats {
    listening_on: String,
    started_at: u64,
    /// Connections currently piped through this tunnel.
    open: AtomicUsize,
    /// Connections accepted since the tunnel started.
    total: AtomicU64,
    healthy: AtomicBool,
    last_error: StdMutex<Option<String>>,
}

impl TunnelStats {
    fn new(listening_on: String) -> Self {
        Self {
            listening_on,
            started_at: now_ms(),
            open: AtomicUsize::new(0),
            total: AtomicU64::new(0),
            healthy: AtomicBool::new(true),
            last_error: StdMutex::new(None),
        }
    }

    /// The tunnel is bound but not usable, and this is why.
    fn fail(&self, msg: impl Into<String>) {
        self.healthy.store(false, Ordering::Relaxed);
        *self.last_error.lock().unwrap() = Some(msg.into());
    }

    /// Traffic flowed again — drop the stale reason.
    fn recover(&self) {
        self.healthy.store(true, Ordering::Relaxed);
        *self.last_error.lock().unwrap() = None;
    }

    fn snapshot(&self) -> TunnelStatus {
        let healthy = self.healthy.load(Ordering::Relaxed);
        TunnelStatus {
            state: if healthy {
                TunnelState::Active
            } else {
                TunnelState::Error
            },
            listening_on: Some(self.listening_on.clone()),
            started_at: Some(self.started_at),
            uptime_seconds: Some(now_ms().saturating_sub(self.started_at) / 1000),
            connections: self.open.load(Ordering::Relaxed),
            total_connections: self.total.load(Ordering::Relaxed),
            last_error: self.last_error.lock().unwrap().clone(),
        }
    }
}

/// The runtime half of a rule, as handed to the UI and to MCP.
#[derive(Debug, Clone, Serialize)]
pub struct TunnelStatus {
    pub state: TunnelState,
    pub listening_on: Option<String>,
    pub started_at: Option<u64>,
    pub uptime_seconds: Option<u64>,
    pub connections: usize,
    pub total_connections: u64,
    pub last_error: Option<String>,
}

impl TunnelStatus {
    /// Not running. A remembered failure is the difference between "off" and
    /// "tried and could not" — the whole point of keeping it around.
    fn stopped(last_error: Option<String>) -> Self {
        Self {
            state: if last_error.is_some() {
                TunnelState::Error
            } else {
                TunnelState::Inactive
            },
            listening_on: None,
            started_at: None,
            uptime_seconds: None,
            connections: 0,
            total_connections: 0,
            last_error,
        }
    }
}

/// A rule plus everything known about what it is doing.
#[derive(Debug, Clone, Serialize)]
pub struct PortForwardingRuleView {
    pub id: String,
    pub name: String,
    pub host_id: String,
    /// Resolved for display — a rule whose host was deleted shows `null`.
    pub host_name: Option<String>,
    pub bind_address: String,
    pub local_port: u16,
    pub destination_address: String,
    pub destination_port: u16,
    /// Live: forwarding right now. Kept as `active` so older clients that read
    /// this field get the truth rather than the stored intent.
    pub active: bool,
    /// A listener is bound for this rule — true even while `status` is `error`,
    /// because a tunnel whose SSH session died still holds the local port. The
    /// UI needs the distinction to offer "stop" rather than a "start" that
    /// would only answer "already running".
    pub running: bool,
    /// Persisted intent, re-armed on boot.
    pub enabled: bool,
    pub status: TunnelState,
    pub listening_on: Option<String>,
    pub started_at: Option<u64>,
    pub uptime_seconds: Option<u64>,
    pub connections: usize,
    pub total_connections: u64,
    pub last_error: Option<String>,
}

impl PortForwardingRuleView {
    fn new(rule: PortForwardingRule, host_name: Option<String>, status: TunnelStatus) -> Self {
        Self {
            id: rule.id,
            name: rule.name,
            host_id: rule.host_id,
            host_name,
            bind_address: rule.bind_address,
            local_port: rule.local_port,
            destination_address: rule.destination_address,
            destination_port: rule.destination_port,
            active: status.state == TunnelState::Active,
            running: status.listening_on.is_some(),
            enabled: rule.active,
            status: status.state,
            listening_on: status.listening_on,
            started_at: status.started_at,
            uptime_seconds: status.uptime_seconds,
            connections: status.connections,
            total_connections: status.total_connections,
            last_error: status.last_error,
        }
    }

    /// One line for MCP output and logs: `9443: 127.0.0.1:9443 → 127.0.0.1:6443 via server-190`.
    pub fn summary(&self) -> String {
        format!(
            "{}: {}:{} → {}:{} via {}",
            self.name,
            self.bind_address,
            self.local_port,
            self.destination_address,
            self.destination_port,
            self.host_name.as_deref().unwrap_or("<missing host>")
        )
    }
}

struct Tunnel {
    /// Dropping or firing this ends the accept loop.
    cancel: tokio::sync::oneshot::Sender<()>,
    stats: Arc<TunnelStats>,
}

pub struct PortForwardingManager {
    tunnels: Mutex<HashMap<String, Tunnel>>,
    /// Why a rule that is *not* running last failed. Without this a tunnel that
    /// could not bind its port, or whose host refused the connection, would be
    /// indistinguishable from one nobody ever started.
    failures: Mutex<HashMap<String, String>>,
}

impl PortForwardingManager {
    pub fn new() -> Self {
        Self {
            tunnels: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
        }
    }

    pub async fn is_running(&self, rule_id: &str) -> bool {
        self.tunnels.lock().await.contains_key(rule_id)
    }

    pub async fn status(&self, rule_id: &str) -> TunnelStatus {
        if let Some(tunnel) = self.tunnels.lock().await.get(rule_id) {
            return tunnel.stats.snapshot();
        }
        TunnelStatus::stopped(self.failures.lock().await.get(rule_id).cloned())
    }

    async fn record_failure(&self, rule_id: &str, msg: String) -> String {
        self.failures
            .lock()
            .await
            .insert(rule_id.to_string(), msg.clone());
        msg
    }

    /// Forget a rule entirely (it was deleted).
    pub async fn forget(&self, rule_id: &str) {
        self.failures.lock().await.remove(rule_id);
    }

    /// Bind the local port and prove the SSH side works before reporting
    /// success. Returns the live status, or the reason it could not start —
    /// which is also remembered, so a later `status` call can still explain it.
    pub async fn start_tunnel(
        &self,
        state: Arc<AppState>,
        rule: PortForwardingRule,
    ) -> Result<TunnelStatus, String> {
        if self.is_running(&rule.id).await {
            return Err(format!("Tunnel '{}' is already running", rule.name));
        }

        let host = match state.hosts.get(&rule.host_id) {
            Some(h) => h,
            None => {
                let msg = format!(
                    "SSH host '{}' for rule '{}' no longer exists — edit the rule and pick a host.",
                    rule.host_id, rule.name
                );
                return Err(self.record_failure(&rule.id, msg).await);
            }
        };

        let bind_addr = format!("{}:{}", rule.bind_address, rule.local_port);
        let listener = match TcpListener::bind(&bind_addr).await {
            Ok(l) => l,
            Err(e) => {
                let msg = format!("Cannot bind {}: {}", bind_addr, e);
                return Err(self.record_failure(&rule.id, msg).await);
            }
        };

        // Connect before declaring the tunnel up. This is the check that makes
        // "Active" mean something: a rule whose host is down now fails here,
        // loudly, instead of at the first connection nobody is watching.
        let client = match connect_host(&state, &host).await {
            Err(e) => {
                let msg = format!(
                    "SSH connection to {}@{}:{} failed: {}",
                    host.user, host.host, host.port, e
                );
                return Err(self.record_failure(&rule.id, msg).await);
            }
            Ok(c) => c,
        };

        let label = tunnel_label(&rule, &host);
        let stats = Arc::new(TunnelStats::new(bind_addr.clone()));
        let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

        self.tunnels.lock().await.insert(
            rule.id.clone(),
            Tunnel {
                cancel: tx,
                stats: stats.clone(),
            },
        );
        self.failures.lock().await.remove(&rule.id);

        let ssh = Arc::new(Mutex::new(client));
        let snapshot = stats.snapshot();

        {
            let stats = stats.clone();
            let state = state.clone();
            let rule = rule.clone();
            let host = host.clone();
            let label = label.clone();

            tokio::spawn(async move {
                let mut health = tokio::time::interval_at(
                    tokio::time::Instant::now() + HEALTH_EVERY,
                    HEALTH_EVERY,
                );

                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            match accepted {
                                Ok((stream, peer)) => {
                                    stats.total.fetch_add(1, Ordering::Relaxed);
                                    tokio::spawn(pipe_connection(
                                        stream,
                                        peer,
                                        ssh.clone(),
                                        stats.clone(),
                                        state.clone(),
                                        rule.clone(),
                                        host.clone(),
                                        label.clone(),
                                    ));
                                }
                                Err(e) => {
                                    stats.fail(format!("Accept failed: {}", e));
                                    crate::logs::error(
                                        &state.log_store,
                                        "tunnel",
                                        "accept_failed",
                                        Some(label.clone()),
                                        format!("Cannot accept on {}: {}", stats.listening_on, e),
                                    );
                                    // Back off rather than spin on a listener
                                    // that keeps erroring (fd exhaustion).
                                    tokio::time::sleep(Duration::from_millis(500)).await;
                                }
                            }
                        }
                        _ = health.tick() => {
                            if ssh.lock().await.handle.is_closed() {
                                match connect_host(&state, &host).await {
                                    Ok(fresh) => {
                                        *ssh.lock().await = fresh;
                                        stats.recover();
                                        crate::logs::info(
                                            &state.log_store,
                                            "tunnel",
                                            "reconnected",
                                            Some(label.clone()),
                                            "SSH session had closed; reconnected",
                                        );
                                    }
                                    Err(e) => {
                                        stats.fail(format!("SSH session lost: {}", e));
                                        crate::logs::warn(
                                            &state.log_store,
                                            "tunnel",
                                            "session_lost",
                                            Some(label.clone()),
                                            format!("SSH session closed and reconnect failed: {}", e),
                                        );
                                    }
                                }
                            }
                        }
                        _ = &mut rx => break,
                    }
                }

                state
                    .port_forwarding_manager
                    .clear_if_current(&rule.id, &stats)
                    .await;
                crate::logs::info(
                    &state.log_store,
                    "tunnel",
                    "stopped",
                    Some(label),
                    format!(
                        "Tunnel stopped after {} connection(s)",
                        stats.total.load(Ordering::Relaxed)
                    ),
                );
            });
        }

        crate::logs::info(
            &state.log_store,
            "tunnel",
            "started",
            Some(label),
            format!(
                "Forwarding {} → {}:{}",
                bind_addr, rule.destination_address, rule.destination_port
            ),
        );

        Ok(snapshot)
    }

    /// Ask a running tunnel to stop. Returns false when it was not running.
    pub async fn stop_tunnel(&self, rule_id: &str) -> bool {
        let tunnel = self.tunnels.lock().await.remove(rule_id);
        match tunnel {
            Some(t) => {
                let _ = t.cancel.send(());
                true
            }
            None => false,
        }
    }

    /// Drop the map entry for a tunnel whose loop has exited — but only if the
    /// entry is still *that* tunnel. Stop-then-start on the same rule can put a
    /// new tunnel in the map before the old task finishes unwinding.
    async fn clear_if_current(&self, rule_id: &str, stats: &Arc<TunnelStats>) {
        let mut tunnels = self.tunnels.lock().await;
        if let Some(t) = tunnels.get(rule_id) {
            if Arc::ptr_eq(&t.stats, stats) {
                tunnels.remove(rule_id);
            }
        }
    }
}

fn tunnel_label(rule: &PortForwardingRule, host: &Host) -> String {
    format!(
        "{} ({}:{} → {}:{} via {})",
        rule.name,
        rule.bind_address,
        rule.local_port,
        rule.destination_address,
        rule.destination_port,
        host.name
    )
}

/// Open an SSH session, resolving credentials the same way the terminal does
/// (host password, or a keychain password / private key). Shared with the
/// remote-filesystem API so an editor connection authenticates identically.
pub async fn connect_host(state: &AppState, host: &Host) -> Result<SshClient, String> {
    let mut password = host.password.clone();
    let mut key_pair = None;

    if let Some(keychain_id) = &host.keychain_id {
        match state.keychain.get(keychain_id) {
            Some(item) => match item.item_type {
                crate::keychain::KeychainItemType::Password => password = Some(item.value.clone()),
                crate::keychain::KeychainItemType::PrivateKey => {
                    key_pair = Some(
                        russh_keys::decode_secret_key(&item.value, password.as_deref())
                            .map_err(|e| format!("cannot decode key '{}': {}", item.name, e))?,
                    );
                }
            },
            None => return Err(format!("keychain item '{}' not found", keychain_id)),
        }
    }

    SshClient::connect(
        &host.host,
        host.port,
        &host.user,
        password.as_deref(),
        key_pair,
        Some(host.id.clone()),
    )
    .await
    .map_err(|e| e.to_string())
}

async fn open_channel(
    ssh: &Arc<Mutex<SshClient>>,
    rule: &PortForwardingRule,
    peer: &SocketAddr,
) -> Result<Channel<Msg>, String> {
    let client = ssh.lock().await;
    client
        .handle
        .channel_open_direct_tcpip(
            rule.destination_address.clone(),
            rule.destination_port as u32,
            peer.ip().to_string(),
            peer.port() as u32,
        )
        .await
        .map_err(|e| {
            format!(
                "cannot open channel to {}:{}: {}",
                rule.destination_address, rule.destination_port, e
            )
        })
}

/// Pipe one accepted TCP connection through the tunnel's SSH session.
#[allow(clippy::too_many_arguments)]
async fn pipe_connection(
    mut stream: tokio::net::TcpStream,
    peer: SocketAddr,
    ssh: Arc<Mutex<SshClient>>,
    stats: Arc<TunnelStats>,
    state: Arc<AppState>,
    rule: PortForwardingRule,
    host: Host,
    label: String,
) {
    let channel = match open_channel(&ssh, &rule, &peer).await {
        Ok(c) => c,
        Err(first) => {
            // A session that died between health ticks is the common case, and
            // failing the user's connection while the status still reads
            // "active" is exactly the lie this module exists to stop. One
            // reconnect, one retry, then say what went wrong.
            match connect_host(&state, &host).await {
                Ok(fresh) => {
                    *ssh.lock().await = fresh;
                    match open_channel(&ssh, &rule, &peer).await {
                        Ok(c) => c,
                        Err(second) => {
                            stats.fail(second.clone());
                            crate::logs::error(
                                &state.log_store,
                                "tunnel",
                                "forward_failed",
                                Some(label),
                                format!("Reconnected but {}", second),
                            );
                            return;
                        }
                    }
                }
                Err(reconnect) => {
                    let msg = format!("{}; reconnect failed: {}", first, reconnect);
                    stats.fail(msg.clone());
                    crate::logs::error(
                        &state.log_store,
                        "tunnel",
                        "forward_failed",
                        Some(label),
                        msg,
                    );
                    return;
                }
            }
        }
    };

    stats.recover();
    stats.open.fetch_add(1, Ordering::Relaxed);
    let mut channel_stream = channel.into_stream();
    let result = tokio::io::copy_bidirectional(&mut stream, &mut channel_stream).await;
    stats.open.fetch_sub(1, Ordering::Relaxed);

    if let Err(e) = result {
        // Clients hanging up mid-stream is normal; log it, but do not paint the
        // tunnel as broken over one dropped connection.
        crate::logs::warn(
            &state.log_store,
            "tunnel",
            "connection_error",
            Some(label),
            format!("Connection from {} ended: {}", peer, e),
        );
    }
}

/// Re-arm on boot every tunnel the user had switched on. `ssh-manager` runs as
/// a background app, so a restart should not silently leave a rule that says
/// "Active" with nothing listening.
pub async fn restore_saved_tunnels(state: Arc<AppState>) {
    for rule in state.port_forwarding_store.get_all() {
        if !rule.active {
            continue;
        }
        let name = rule.name.clone();
        match state
            .port_forwarding_manager
            .start_tunnel(state.clone(), rule)
            .await
        {
            Ok(_) => {}
            Err(e) => {
                crate::logs::error(
                    &state.log_store,
                    "tunnel",
                    "restore_failed",
                    Some(name.clone()),
                    format!("Could not restore tunnel '{}': {}", name, e),
                );
            }
        }
    }
}

/// Build the view for one rule (status included).
pub async fn view_of(state: &AppState, rule: PortForwardingRule) -> PortForwardingRuleView {
    let host_name = state.hosts.get(&rule.host_id).map(|h| h.name);
    let status = state.port_forwarding_manager.status(&rule.id).await;
    PortForwardingRuleView::new(rule, host_name, status)
}

/// Build views for every rule.
pub async fn all_views(state: &AppState) -> Vec<PortForwardingRuleView> {
    let rules = state.port_forwarding_store.get_all();
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        out.push(view_of(state, rule).await);
    }
    out
}

pub fn port_forwarding_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/port-forwarding", get(list_rules).post(create_rule))
        .route("/port-forwarding/:id", put(update_rule).delete(delete_rule))
        .route("/port-forwarding/:id/start", post(start_rule))
        .route("/port-forwarding/:id/stop", post(stop_rule))
        .route("/port-forwarding/:id/status", get(rule_status))
}

/// Result of a start/stop call. Always carries the rule's status afterwards so
/// the caller never has to guess whether the tunnel came up.
#[derive(Serialize)]
struct ActionResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule: Option<PortForwardingRuleView>,
}

async fn list_rules(State(state): State<Arc<AppState>>) -> Json<Vec<PortForwardingRuleView>> {
    Json(all_views(&state).await)
}

async fn rule_status(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<Json<PortForwardingRuleView>, StatusCode> {
    match state.port_forwarding_store.get(&id) {
        Some(rule) => Ok(Json(view_of(&state, rule).await)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn create_rule(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PortForwardingRule>,
) -> Json<PortForwardingRuleView> {
    let rule = state.port_forwarding_store.add(payload);
    crate::logs::info(
        &state.log_store,
        "ui",
        "port_forward_create",
        Some(rule.name.clone()),
        format!(
            "Created rule {}:{} → {}:{}",
            rule.bind_address, rule.local_port, rule.destination_address, rule.destination_port
        ),
    );

    // A rule saved as enabled should come up now, not on the next restart.
    if rule.active {
        let _ = state
            .port_forwarding_manager
            .start_tunnel(state.clone(), rule.clone())
            .await;
    }

    Json(view_of(&state, rule).await)
}

async fn update_rule(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PortForwardingRule>,
) -> axum::response::Result<Json<PortForwardingRuleView>, StatusCode> {
    // Editing a live tunnel used to leave it forwarding the *old* ports while
    // the UI showed the new ones. Cycle it instead.
    let was_running = state.port_forwarding_manager.stop_tunnel(&id).await;

    let updated = match state.port_forwarding_store.update(&id, payload) {
        Some(r) => r,
        None => return Err(StatusCode::NOT_FOUND),
    };

    if was_running || updated.active {
        state.port_forwarding_store.set_active(&id, true);
        let mut rule = updated.clone();
        rule.active = true;
        let _ = state
            .port_forwarding_manager
            .start_tunnel(state.clone(), rule)
            .await;
    }

    let rule = state
        .port_forwarding_store
        .get(&id)
        .unwrap_or_else(|| updated.clone());
    Ok(Json(view_of(&state, rule).await))
}

async fn delete_rule(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<Json<bool>, StatusCode> {
    state.port_forwarding_manager.stop_tunnel(&id).await;
    state.port_forwarding_manager.forget(&id).await;

    if state.port_forwarding_store.delete(&id) {
        crate::logs::info(
            &state.log_store,
            "ui",
            "port_forward_delete",
            None,
            format!("Deleted port forwarding rule id={}", id),
        );
        Ok(Json(true))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn start_rule(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let rule = match state.port_forwarding_store.get(&id) {
        Some(r) => r,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ActionResponse {
                    ok: false,
                    error: Some(format!("No port forwarding rule with id {}", id)),
                    rule: None,
                }),
            );
        }
    };

    match state
        .port_forwarding_manager
        .start_tunnel(state.clone(), rule.clone())
        .await
    {
        Ok(_) => {
            state.port_forwarding_store.set_active(&id, true);
            let rule = state.port_forwarding_store.get(&id).unwrap_or(rule);
            (
                StatusCode::OK,
                Json(ActionResponse {
                    ok: true,
                    error: None,
                    rule: Some(view_of(&state, rule).await),
                }),
            )
        }
        Err(e) => {
            crate::logs::error(
                &state.log_store,
                "ui",
                "port_forward_start",
                Some(rule.name.clone()),
                format!("Could not start tunnel '{}': {}", rule.name, e),
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(ActionResponse {
                    ok: false,
                    error: Some(e),
                    rule: Some(view_of(&state, rule).await),
                }),
            )
        }
    }
}

async fn stop_rule(Path(id): Path<String>, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rule = match state.port_forwarding_store.get(&id) {
        Some(r) => r,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ActionResponse {
                    ok: false,
                    error: Some(format!("No port forwarding rule with id {}", id)),
                    rule: None,
                }),
            );
        }
    };

    let was_running = state.port_forwarding_manager.stop_tunnel(&id).await;
    state.port_forwarding_store.set_active(&id, false);
    // Stopping on purpose clears any remembered failure — otherwise the rule
    // would sit at "error" forever after one bad start.
    state.port_forwarding_manager.forget(&id).await;

    if was_running {
        crate::logs::info(
            &state.log_store,
            "ui",
            "port_forward_stop",
            Some(rule.name.clone()),
            format!("Stopped tunnel '{}'", rule.name),
        );
    }

    let rule = state.port_forwarding_store.get(&id).unwrap_or(rule);
    (
        StatusCode::OK,
        Json(ActionResponse {
            ok: true,
            error: None,
            rule: Some(view_of(&state, rule).await),
        }),
    )
}
