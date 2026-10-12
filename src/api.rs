use crate::client::SshClient;
use crate::keychain::{KeychainItem, KeychainStore};
use crate::logs::{self, LogEntry, LogStore};
use crate::models::{Host, HostStore};
use crate::security::CommandFilter;
use crate::settings::{Settings, SettingsStore};
use crate::snippets::{Snippet, SnippetStore};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct SshExecuteRequest {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub command: String,
}

#[derive(Serialize)]
pub struct SshExecuteResponse {
    pub output: String,
    pub error: Option<String>,
}

pub struct AppState {
    pub mcp_tx: tokio::sync::broadcast::Sender<String>,
    pub ui_tx: tokio::sync::broadcast::Sender<String>,
    pub filter: CommandFilter,
    pub hosts: HostStore,
    pub keychain: KeychainStore,
    /// Reusable command snippets shown in the right sidebar.
    pub snippets: SnippetStore,
    pub connections: crate::connection::ConnectionManager,
    pub port_forwarding_store: crate::models::PortForwardingStore,
    pub port_forwarding_manager: crate::port_forwarding::PortForwardingManager,
    /// One SFTP session per SSH connection, for the remote-filesystem API.
    pub rfs: crate::rfs::SftpCache,
    pub log_store: Arc<LogStore>,
    pub settings: Arc<SettingsStore>,
}

pub fn api_router() -> Router {
    let allowed_commands_env = std::env::var("ALLOWED_COMMANDS")
        .unwrap_or_else(|_| "ls,pwd,whoami,df,free,uname,cat,echo,ps,top".to_string());

    let allowed: Vec<String> = allowed_commands_env
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let filter = CommandFilter::new(allowed);
    let data_dir = dirs::home_dir()
        .unwrap()
        .join(".senclaw")
        .join("space-apps-data")
        .join("ssh-manager");
    std::fs::create_dir_all(&data_dir).unwrap();
    let hosts_path = data_dir.join("hosts.json");
    let hosts = HostStore::new(hosts_path.to_str().unwrap());
    let keychain_path = data_dir.join("keychain.json");
    let keychain = KeychainStore::new(keychain_path.to_str().unwrap());
    let snippets_path = data_dir.join("snippets.json");
    let snippets = SnippetStore::new(snippets_path.to_str().unwrap());
    let (mcp_tx, _) = tokio::sync::broadcast::channel(100);
    let (ui_tx, _) = tokio::sync::broadcast::channel(100);
    let connections = crate::connection::ConnectionManager::new();

    let port_forwarding_path = data_dir.join("port_forwarding.json");
    let port_forwarding_store =
        crate::models::PortForwardingStore::new(port_forwarding_path.to_str().unwrap());
    let port_forwarding_manager = crate::port_forwarding::PortForwardingManager::new();

    let log_store = Arc::new(LogStore::new());
    logs::info(&log_store, "system", "boot", None, "SSH Manager started");
    let settings = Arc::new(SettingsStore::new(data_dir.join("settings.json")));

    // Background sweep: prune logs by retention setting every 30s.
    {
        let log_store = log_store.clone();
        let settings = settings.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tick.tick().await;
                let secs = settings.log_retention();
                if secs > 0 {
                    let dropped = log_store.prune_older_than(secs * 1000);
                    if dropped > 0 {
                        logs::info(
                            &log_store,
                            "system",
                            "log_prune",
                            None,
                            format!("Auto-pruned {} old log entries", dropped),
                        );
                    }
                }
            }
        });
    }

    let state = Arc::new(AppState {
        filter,
        hosts,
        keychain,
        snippets,
        mcp_tx,
        ui_tx,
        connections,
        port_forwarding_store,
        port_forwarding_manager,
        rfs: crate::rfs::SftpCache::new(),
        log_store,
        settings,
    });

    // ssh-manager is a background app: a tunnel the user switched on should be
    // forwarding again after a restart, not merely remembered as "active".
    {
        let state = state.clone();
        tokio::spawn(async move {
            crate::port_forwarding::restore_saved_tunnels(state).await;
        });
    }

    Router::new()
        .nest("/sftp", crate::sftp_api::sftp_router())
        .nest("/rfs", crate::rfs::rfs_router())
        .merge(crate::port_forwarding::port_forwarding_router())
        .route("/ui-events", get(ui_events_sse))
        .route("/logs", get(list_logs).delete(clear_logs))
        .route("/logs/stream", get(logs_sse))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/execute", post(execute_command))
        .route("/hosts", get(list_hosts).post(create_host))
        .route("/hosts/:id", put(update_host).delete(delete_host))
        .route("/keychain", get(list_keychain).post(create_keychain))
        .route(
            "/keychain/:id",
            put(update_keychain).delete(delete_keychain),
        )
        .route("/snippets", get(list_snippets).post(create_snippet))
        .route(
            "/snippets/:id",
            put(update_snippet).delete(delete_snippet),
        )
        .route(
            "/mcp/sse",
            get(crate::mcp::mcp_sse).post(crate::mcp::mcp_message),
        )
        .route("/mcp/message", post(crate::mcp::mcp_message))
        .route("/ws/terminal/:id", get(ws_terminal_handler))
        .with_state(state)
}

async fn list_hosts(State(state): State<Arc<AppState>>) -> Json<Vec<Host>> {
    Json(state.hosts.get_all())
}

async fn create_host(State(state): State<Arc<AppState>>, Json(host): Json<Host>) -> Json<Host> {
    let added = state.hosts.add(host);
    logs::info(
        &state.log_store,
        "ui",
        "host_create",
        Some(added.name.clone()),
        format!("Created host {}@{}:{}", added.user, added.host, added.port),
    );
    Json(added)
}

async fn update_host(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(host): Json<Host>,
) -> axum::response::Result<Json<Host>, axum::http::StatusCode> {
    if let Some(updated) = state.hosts.update(&id, host) {
        logs::info(
            &state.log_store,
            "ui",
            "host_update",
            Some(updated.name.clone()),
            format!(
                "Updated host {}@{}:{}",
                updated.user, updated.host, updated.port
            ),
        );
        Ok(Json(updated))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

async fn delete_host(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Result<Json<bool>, axum::http::StatusCode> {
    let host_name = state
        .hosts
        .get_all()
        .into_iter()
        .find(|h| h.id == id)
        .map(|h| h.name);
    if state.hosts.delete(&id) {
        logs::info(
            &state.log_store,
            "ui",
            "host_delete",
            host_name,
            format!("Deleted host id={}", id),
        );
        Ok(Json(true))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

async fn execute_command(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SshExecuteRequest>,
) -> Json<SshExecuteResponse> {
    // CommandFilter is disabled for MCP / AI Agent unrestricted access
    let host_label = format!("{}@{}:{}", payload.user, payload.host, payload.port);
    logs::info(
        &state.log_store,
        "ui",
        "exec",
        Some(host_label.clone()),
        format!("Execute: {}", payload.command),
    );

    let mut password = payload.password.clone();
    let mut key_pair = None;

    // MCP currently doesn't provide keychain_id directly in payload unless we updated it.
    // If we wanted to support it, we'd need to add it to SshExecuteRequest.
    // But let's check if the host in HostStore has it, or if payload has it.
    // Since MCP provides host ip/port/user/password directly, we might not have a host in HostStore.
    // However, if the payload matched a host in the store, we could look it up.
    // For now, let's just use what's in payload. If we want MCP to use keychain, we should update SshExecuteRequest to accept keychain_id.
    let mut client = match SshClient::connect(
        &payload.host,
        payload.port,
        &payload.user,
        password.as_deref(),
        key_pair,
        None,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            logs::error(
                &state.log_store,
                "ssh",
                "connect_failed",
                Some(host_label.clone()),
                format!("Connection failed: {}", e),
            );
            return Json(SshExecuteResponse {
                output: String::new(),
                error: Some(format!("Connection failed: {}", e)),
            });
        }
    };

    match client.execute(&payload.command).await {
        Ok(out) => {
            logs::info(
                &state.log_store,
                "ssh",
                "exec_ok",
                Some(host_label.clone()),
                format!("Output: {} bytes", out.len()),
            );
            Json(SshExecuteResponse {
                output: out,
                error: None,
            })
        }
        Err(e) => {
            logs::error(
                &state.log_store,
                "ssh",
                "exec_failed",
                Some(host_label.clone()),
                format!("Execution failed: {}", e),
            );
            Json(SshExecuteResponse {
                output: String::new(),
                error: Some(format!("Execution failed: {}", e)),
            })
        }
    }
}

/// The terminal size the browser passes when it opens the socket, so the PTY
/// starts at the right width instead of being corrected after the first prompt.
#[derive(serde::Deserialize)]
pub struct TerminalSizeQuery {
    pub cols: Option<u32>,
    pub rows: Option<u32>,
}

async fn ws_terminal_handler(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    Query(size): Query<TerminalSizeQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let default = crate::client::TerminalSize::default();
    let size = crate::client::TerminalSize {
        cols: size.cols.unwrap_or(default.cols),
        rows: size.rows.unwrap_or(default.rows),
    };
    let host_opt = state.hosts.get_all().into_iter().find(|h| h.id == id);

    ws.on_upgrade(move |socket| async move {
        if let Some(host) = host_opt {
            let host_label = format!("{}@{}:{}", host.user, host.host, host.port);
            logs::info(
                &state.log_store,
                "ui",
                "terminal_open",
                Some(host.name.clone()),
                format!("Opening interactive shell to {}", host_label),
            );
            let mut password = host.password.clone();
            let mut key_pair = None;

            if let Some(kid) = &host.keychain_id {
                if let Some(kitem) = state.keychain.get(kid) {
                    match kitem.item_type {
                        crate::keychain::KeychainItemType::Password => {
                            password = Some(kitem.value);
                        }
                        crate::keychain::KeychainItemType::PrivateKey => {
                            // russh_keys::decode_secret_key requires the key and an optional password
                            if let Ok(kp) = russh_keys::decode_secret_key(
                                kitem.value.as_str(),
                                password.as_deref(),
                            ) {
                                key_pair = Some(kp);
                            } else {
                                eprintln!("Failed to parse private key");
                            }
                        }
                    }
                }
            }

            match SshClient::connect(
                &host.host,
                host.port,
                &host.user,
                password.as_deref(),
                key_pair,
                None,
            )
            .await
            {
                Ok(mut client) => {
                    logs::info(
                        &state.log_store,
                        "ssh",
                        "terminal_connected",
                        Some(host.name.clone()),
                        format!("SSH session established → {}", host_label),
                    );
                    if let Err(e) = client.interactive_shell(socket, size).await {
                        logs::error(
                            &state.log_store,
                            "ssh",
                            "terminal_error",
                            Some(host.name.clone()),
                            format!("Interactive shell error: {}", e),
                        );
                        eprintln!("Interactive shell error: {}", e);
                    }
                    logs::info(
                        &state.log_store,
                        "ssh",
                        "terminal_closed",
                        Some(host.name.clone()),
                        format!("SSH session ended → {}", host_label),
                    );
                }
                Err(e) => {
                    logs::error(
                        &state.log_store,
                        "ssh",
                        "connect_failed",
                        Some(host.name.clone()),
                        format!("SSH connection failed → {}: {}", host_label, e),
                    );
                    eprintln!("SSH Connection failed: {}", e);
                    let mut s = socket;
                    let _ = s
                        .send(axum::extract::ws::Message::Text(format!(
                            "\\r\\n\\x1b[31mSSH Connection Failed: {}\\x1b[0m\\r\\n",
                            e
                        )))
                        .await;
                }
            }
        } else {
            logs::warn(
                &state.log_store,
                "ui",
                "terminal_open",
                None,
                format!("Host not found: id={}", id),
            );
            let mut s = socket;
            let _ = s
                .send(axum::extract::ws::Message::Text(
                    "\\r\\n\\x1b[31mHost not found.\\x1b[0m\\r\\n".to_string(),
                ))
                .await;
        }
    })
}

async fn list_keychain(State(state): State<Arc<AppState>>) -> Json<Vec<KeychainItem>> {
    Json(state.keychain.get_all())
}

async fn create_keychain(
    State(state): State<Arc<AppState>>,
    Json(item): Json<KeychainItem>,
) -> Json<KeychainItem> {
    let added = state.keychain.add(item);
    logs::info(
        &state.log_store,
        "ui",
        "keychain_create",
        None,
        format!("Created keychain item: {}", added.name),
    );
    Json(added)
}

async fn update_keychain(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(item): Json<KeychainItem>,
) -> axum::response::Result<Json<KeychainItem>, axum::http::StatusCode> {
    if let Some(updated) = state.keychain.update(&id, item) {
        logs::info(
            &state.log_store,
            "ui",
            "keychain_update",
            None,
            format!("Updated keychain item: {}", updated.name),
        );
        Ok(Json(updated))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

async fn delete_keychain(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Result<Json<bool>, axum::http::StatusCode> {
    if state.keychain.delete(&id) {
        logs::info(
            &state.log_store,
            "ui",
            "keychain_delete",
            None,
            format!("Deleted keychain item id={}", id),
        );
        Ok(Json(true))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

async fn list_snippets(State(state): State<Arc<AppState>>) -> Json<Vec<Snippet>> {
    Json(state.snippets.get_all())
}

async fn create_snippet(
    State(state): State<Arc<AppState>>,
    Json(item): Json<Snippet>,
) -> Json<Snippet> {
    let added = state.snippets.add(item);
    logs::info(
        &state.log_store,
        "ui",
        "snippet_create",
        None,
        format!("Created snippet: {}", added.name),
    );
    Json(added)
}

async fn update_snippet(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(item): Json<Snippet>,
) -> axum::response::Result<Json<Snippet>, axum::http::StatusCode> {
    if let Some(updated) = state.snippets.update(&id, item) {
        logs::info(
            &state.log_store,
            "ui",
            "snippet_update",
            None,
            format!("Updated snippet: {}", updated.name),
        );
        Ok(Json(updated))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

async fn delete_snippet(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Result<Json<bool>, axum::http::StatusCode> {
    if state.snippets.delete(&id) {
        logs::info(
            &state.log_store,
            "ui",
            "snippet_delete",
            None,
            format!("Deleted snippet id={}", id),
        );
        Ok(Json(true))
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

#[derive(Deserialize)]
pub struct LogQuery {
    pub limit: Option<usize>,
}

async fn list_logs(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<LogQuery>,
) -> Json<Vec<LogEntry>> {
    Json(state.log_store.list(q.limit.unwrap_or(500)))
}

async fn clear_logs(State(state): State<Arc<AppState>>) -> Json<bool> {
    state.log_store.clear();
    logs::info(
        &state.log_store,
        "ui",
        "logs_clear",
        None,
        "Log buffer cleared",
    );
    Json(true)
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<Settings> {
    Json(state.settings.get())
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Settings>,
) -> Json<Settings> {
    let saved = state.settings.set(body);
    logs::info(
        &state.log_store,
        "ui",
        "settings_update",
        None,
        format!(
            "Settings updated: theme={} retention={}s ssh_policy={}",
            saved.theme, saved.log_retention_seconds, saved.ssh_command_policy
        ),
    );
    Json(saved)
}

async fn logs_sse(
    State(state): State<Arc<AppState>>,
) -> axum::response::sse::Sse<
    impl futures_util::stream::Stream<
        Item = Result<axum::response::sse::Event, std::convert::Infallible>,
    >,
> {
    let mut rx = state.log_store.subscribe();
    let stream = async_stream::stream! {
        while let Ok(entry) = rx.recv().await {
            if let Ok(data) = serde_json::to_string(&entry) {
                yield Ok(axum::response::sse::Event::default().event("log").data(data));
            }
        }
    };
    axum::response::sse::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

pub async fn ui_events_sse(
    State(state): State<Arc<AppState>>,
) -> axum::response::sse::Sse<
    impl futures_util::stream::Stream<
        Item = Result<axum::response::sse::Event, std::convert::Infallible>,
    >,
> {
    let mut rx = state.ui_tx.subscribe();
    let stream = async_stream::stream! {
        while let Ok(msg) = rx.recv().await {
            yield Ok(axum::response::sse::Event::default().event("ui-event").data(msg));
        }
    };
    axum::response::sse::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}
