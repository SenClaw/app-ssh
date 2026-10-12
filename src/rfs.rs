//! Remote filesystem — the file operations a code editor needs, performed on
//! the far side of an SSH connection.
//!
//! This is what lets `code-ide` open a folder that lives on a server: it keeps
//! the editor, and asks this app (through the daemon's app proxy) for the
//! bytes. The SSH host list, the credentials and the connection pool already
//! live here, so nothing is duplicated over there.
//!
//! The shapes mirror `code-ide`'s local workspace deliberately — `TreeEntry`,
//! `FileContent` and `SearchHit` are the same fields it already serves from
//! `workspace.rs`, and every `path` is workspace-relative — so the provider on
//! the other side is a swap, not a translation layer.
//!
//! Two deliberate choices:
//!
//! * **Saves are written in place** (`SFTP write` truncates and rewrites the
//!   same inode) rather than written to a temp file and renamed. SFTP v3 rename
//!   fails when the destination exists, and delete-then-rename would hand the
//!   file a fresh mode and owner — editing `/etc/nginx/nginx.conf` should not
//!   silently change its permissions.
//! * **Reads and writes go through one cached SFTP session per connection.**
//!   Opening a session costs a channel plus a version exchange; doing that on
//!   every keystroke-save is a round trip nobody needs. A dead session is
//!   evicted and retried once, which is also how a dropped connection heals.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::api::AppState;

/// Directory entries the explorer and search never show.
const HARD_IGNORE: &[&str] = &[".git", "node_modules", "target", "dist", ".DS_Store"];

/// Refuse to open anything larger as text (same limit as the local workspace).
const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SEARCH_MATCHES: usize = 300;
const MAX_FILE_LIST: usize = 6000;

// ── wire shapes (mirror code-ide's workspace.rs) ────────────────────────────

#[derive(Serialize)]
pub struct TreeEntry {
    pub name: String,
    /// Workspace-relative, forward slashes.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Serialize)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub too_large: bool,
    pub binary: bool,
    pub size: u64,
}

#[derive(Serialize)]
pub struct SearchHit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

#[derive(Deserialize)]
pub struct ConnectBody {
    pub host_id: String,
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    /// Absolute remote path. Empty → the login directory.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct RootPathQuery {
    pub root: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Deserialize)]
pub struct RootQuery {
    pub root: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub root: String,
    pub q: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct SaveBody {
    pub root: String,
    pub path: String,
    pub content: String,
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub root: String,
    pub path: String,
    #[serde(default)]
    pub dir: bool,
}

#[derive(Deserialize)]
pub struct RenameBody {
    pub root: String,
    pub from: String,
    pub to: String,
}

#[derive(Deserialize)]
pub struct DeleteBody {
    pub root: String,
    pub path: String,
}

// ── errors ─────────────────────────────────────────────────────────────────

/// Every failure carries a sentence the editor can show. A silent 500 here
/// surfaces as an editor that just does not save.
pub struct RfsError(StatusCode, String);

impl RfsError {
    fn bad(msg: impl Into<String>) -> Self {
        RfsError(StatusCode::BAD_REQUEST, msg.into())
    }
    fn gone(msg: impl Into<String>) -> Self {
        RfsError(StatusCode::NOT_FOUND, msg.into())
    }
    fn upstream(msg: impl Into<String>) -> Self {
        RfsError(StatusCode::BAD_GATEWAY, msg.into())
    }
}

impl IntoResponse for RfsError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type RfsResult<T> = Result<Json<T>, RfsError>;

// ── SFTP session cache ─────────────────────────────────────────────────────

#[derive(Default)]
pub struct SftpCache {
    sessions: Mutex<HashMap<String, Arc<SftpSession>>>,
}

impl SftpCache {
    pub fn new() -> Self {
        Self::default()
    }

    async fn evict(&self, conn_id: &str) {
        self.sessions.lock().await.remove(conn_id);
    }

    /// Drop the cached session for a connection that is going away.
    pub async fn forget(&self, conn_id: &str) {
        self.evict(conn_id).await;
    }
}

/// The cached SFTP session for a connection, opening one if needed.
async fn sftp_for(state: &AppState, conn_id: &str) -> Result<Arc<SftpSession>, RfsError> {
    if let Some(s) = state.rfs.sessions.lock().await.get(conn_id) {
        return Ok(s.clone());
    }

    let client_arc = state.connections.get(conn_id).await.ok_or_else(|| {
        RfsError::gone(format!(
            "SSH connection '{}' is not open — connect again.",
            conn_id
        ))
    })?;

    let session = {
        let mut client = client_arc.lock().await;
        client
            .get_sftp()
            .await
            .map_err(|e| RfsError::upstream(format!("Cannot start SFTP: {}", e)))?
    };

    let session = Arc::new(session);
    state
        .rfs
        .sessions
        .lock()
        .await
        .insert(conn_id.to_string(), session.clone());
    Ok(session)
}

/// Run an SFTP operation, retrying once on a fresh session. A session that
/// died with its connection would otherwise turn every later call into an
/// error until the user reconnected by hand.
async fn with_sftp<T, F, Fut>(state: &AppState, conn_id: &str, op: F) -> Result<T, RfsError>
where
    F: Fn(Arc<SftpSession>) -> Fut,
    Fut: std::future::Future<Output = Result<T, russh_sftp::client::error::Error>>,
{
    let session = sftp_for(state, conn_id).await?;
    match op(session).await {
        Ok(v) => Ok(v),
        Err(first) => {
            state.rfs.evict(conn_id).await;
            let fresh = sftp_for(state, conn_id).await.map_err(|_| {
                RfsError::upstream(format!("SFTP failed and could not reconnect: {}", first))
            })?;
            op(fresh).await.map_err(|e| RfsError::upstream(e.to_string()))
        }
    }
}

/// Run a shell command on the far side (search, git, find).
async fn exec(state: &AppState, conn_id: &str, cmd: String) -> Result<String, RfsError> {
    let client_arc = state.connections.get(conn_id).await.ok_or_else(|| {
        RfsError::gone(format!(
            "SSH connection '{}' is not open — connect again.",
            conn_id
        ))
    })?;
    let mut client = client_arc.lock().await;
    client
        .execute(&cmd)
        .await
        .map_err(|e| RfsError::upstream(format!("Remote command failed: {}", e)))
}

// ── paths ──────────────────────────────────────────────────────────────────

/// Single-quote a value for the remote shell.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Join a workspace-relative path onto the root, lexically, refusing anything
/// that climbs out. There is no `canonicalize` to lean on at the other end of a
/// socket, so this is the only thing standing between a typo and `/etc`.
fn join_remote(root: &str, rel: &str) -> Result<String, RfsError> {
    if root.trim().is_empty() {
        return Err(RfsError::bad("No workspace root given"));
    }
    if rel.contains('\0') || root.contains('\0') {
        return Err(RfsError::bad("Path contains a NUL byte"));
    }

    let root = root.trim_end_matches('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in rel.trim().trim_start_matches('/').split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(RfsError::bad(format!("Path escapes the workspace: {}", rel)));
                }
            }
            s => parts.push(s),
        }
    }

    if parts.is_empty() {
        Ok(root.to_string())
    } else {
        Ok(format!("{}/{}", root, parts.join("/")))
    }
}

/// Strip the workspace root back off an absolute remote path.
fn rel_of(root: &str, abs: &str) -> String {
    let root = root.trim_end_matches('/');
    abs.strip_prefix(root)
        .map(|r| r.trim_start_matches('/').to_string())
        .unwrap_or_else(|| abs.to_string())
}

fn parent_of(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
        None => None,
    }
}

/// Write a whole file, in place.
///
/// Not `SftpSession::write`: that opens with `WRITE` alone, which fails on a
/// file that does not exist yet **and**, worse, leaves the tail of the old
/// contents behind when the new text is shorter. `CREATE | TRUNCATE` is what an
/// editor's save actually means. In place rather than temp-and-rename so the
/// file keeps its mode and owner — see the module docs.
async fn write_whole(
    session: &SftpSession,
    path: &str,
    data: &[u8],
) -> Result<(), russh_sftp::client::error::Error> {
    let mut file = session
        .open_with_flags(
            path.to_string(),
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )
        .await?;
    file.write_all(data).await?;
    file.shutdown().await?;
    Ok(())
}

/// `mkdir -p` over SFTP: create each missing ancestor in turn.
async fn mkdir_p(session: &SftpSession, path: &str) -> Result<(), russh_sftp::client::error::Error> {
    let mut built = String::new();
    for seg in path.trim_start_matches('/').split('/') {
        if seg.is_empty() {
            continue;
        }
        built.push('/');
        built.push_str(seg);
        if !session.try_exists(built.clone()).await.unwrap_or(false) {
            // Racing another creator is fine; only a real failure matters, and
            // the next try_exists would have caught it.
            let _ = session.create_dir(built.clone()).await;
        }
    }
    Ok(())
}

/// A label for the logs: which host this connection is on.
async fn conn_label(state: &AppState, conn_id: &str) -> Option<String> {
    let client = state.connections.get(conn_id).await?;
    let host_id = client.lock().await.host_id.clone()?;
    state.hosts.get(&host_id).map(|h| h.name)
}

// ── router ─────────────────────────────────────────────────────────────────

pub fn rfs_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/hosts", get(hosts))
        .route("/connect", post(connect))
        .route("/:conn/ping", get(ping))
        .route("/:conn/browse", get(browse))
        .route("/:conn/tree", get(tree))
        .route("/:conn/file", get(file))
        .route("/:conn/files", get(files))
        .route("/:conn/save", post(save))
        .route("/:conn/create", post(create))
        .route("/:conn/rename", post(rename))
        .route("/:conn/delete", post(remove))
        .route("/:conn/search", get(search))
        .route("/:conn/git-status", get(git_status))
}

/// The saved hosts an editor can open a folder on — never the credentials.
/// `/api/hosts` serves the stored record whole, password and all; another app
/// picking a server to connect to has no business receiving that.
async fn hosts(State(state): State<Arc<AppState>>) -> RfsResult<Vec<serde_json::Value>> {
    Ok(Json(
        state
            .hosts
            .get_all()
            .into_iter()
            .map(|h| {
                json!({
                    "id": h.id,
                    "name": h.name,
                    "host": h.host,
                    "port": h.port,
                    "user": h.user,
                    "tags": h.tags,
                })
            })
            .collect(),
    ))
}

/// Open an SSH connection for a saved host and report where its login lands,
/// so the folder picker has somewhere to start.
async fn connect(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ConnectBody>,
) -> RfsResult<serde_json::Value> {
    let host = state.hosts.get(&body.host_id).ok_or_else(|| {
        RfsError::gone(format!(
            "No saved SSH host with id '{}' — pick one from the host list.",
            body.host_id
        ))
    })?;

    let client = crate::port_forwarding::connect_host(&state, &host)
        .await
        .map_err(|e| {
            RfsError::upstream(format!(
                "SSH connection to {}@{}:{} failed: {}",
                host.user, host.host, host.port, e
            ))
        })?;

    let conn_id = state.connections.add(client).await;

    // The login directory, so "open a folder" starts somewhere useful.
    let home = with_sftp(&state, &conn_id, |s| async move { s.canonicalize(".").await })
        .await
        .unwrap_or_else(|_| "/".to_string());

    crate::logs::info(
        &state.log_store,
        "rfs",
        "connect",
        Some(host.name.clone()),
        format!("Editor connected to {}@{}:{}", host.user, host.host, host.port),
    );

    Ok(Json(json!({
        "conn_id": conn_id,
        "host_id": host.id,
        "host": host.host,
        "user": host.user,
        "name": host.name,
        "home": home,
    })))
}

/// Is this connection still usable? The editor polls it to keep its badge honest.
async fn ping(
    Path(conn): Path<String>,
    State(state): State<Arc<AppState>>,
) -> RfsResult<serde_json::Value> {
    let alive = match state.connections.get(&conn).await {
        Some(c) => !c.lock().await.handle.is_closed(),
        None => false,
    };
    Ok(Json(json!({ "alive": alive })))
}

/// Directories under an absolute remote path — the folder picker.
async fn browse(
    Path(conn): Path<String>,
    Query(q): Query<BrowseQuery>,
    State(state): State<Arc<AppState>>,
) -> RfsResult<serde_json::Value> {
    let requested = q.path.unwrap_or_default();
    let path = if requested.trim().is_empty() {
        with_sftp(&state, &conn, |s| async move { s.canonicalize(".").await })
            .await
            .unwrap_or_else(|_| "/".to_string())
    } else {
        requested.trim().to_string()
    };

    let listing = {
        let p = path.clone();
        with_sftp(&state, &conn, move |s| {
            let p = p.clone();
            async move { s.read_dir(p).await }
        })
        .await
        .map_err(|e| RfsError(e.0, format!("Cannot list {}: {}", path, e.1)))?
    };

    let mut dirs: Vec<serde_json::Value> = listing
        .filter(|e| e.metadata().is_dir())
        .map(|e| json!({ "name": e.file_name(), "path": e.path() }))
        .filter(|d| d["name"].as_str() != Some(".") && d["name"].as_str() != Some(".."))
        .collect();
    dirs.sort_by_key(|d| d["name"].as_str().unwrap_or("").to_lowercase());

    Ok(Json(json!({
        "path": path,
        "parent": parent_of(&path),
        "dirs": dirs,
    })))
}

/// Immediate children of a workspace directory. Dirs first, then files.
async fn tree(
    Path(conn): Path<String>,
    Query(q): Query<RootPathQuery>,
    State(state): State<Arc<AppState>>,
) -> RfsResult<Vec<TreeEntry>> {
    let abs = join_remote(&q.root, &q.path)?;
    let listing = {
        let p = abs.clone();
        with_sftp(&state, &conn, move |s| {
            let p = p.clone();
            async move { s.read_dir(p).await }
        })
        .await
        .map_err(|e| RfsError(e.0, format!("Cannot list {}: {}", abs, e.1)))?
    };

    let mut out: Vec<TreeEntry> = listing
        .filter(|e| {
            let n = e.file_name();
            n != "." && n != ".." && !HARD_IGNORE.contains(&n.as_str())
        })
        .map(|e| {
            let meta = e.metadata();
            let is_dir = meta.is_dir();
            TreeEntry {
                name: e.file_name(),
                path: rel_of(&q.root, &e.path()),
                is_dir,
                size: if is_dir { 0 } else { meta.len() },
            }
        })
        .collect();

    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(Json(out))
}

/// Read a file as text, with the same binary/size guards as a local workspace.
async fn file(
    Path(conn): Path<String>,
    Query(q): Query<RootPathQuery>,
    State(state): State<Arc<AppState>>,
) -> RfsResult<FileContent> {
    let abs = join_remote(&q.root, &q.path)?;

    let meta = {
        let p = abs.clone();
        with_sftp(&state, &conn, move |s| {
            let p = p.clone();
            async move { s.metadata(p).await }
        })
        .await
        .map_err(|e| RfsError(e.0, format!("Cannot stat {}: {}", q.path, e.1)))?
    };

    if meta.is_dir() {
        return Err(RfsError::bad(format!("{} is a directory", q.path)));
    }
    let size = meta.len();

    if size > MAX_TEXT_BYTES {
        return Ok(Json(FileContent {
            path: q.path,
            content: String::new(),
            too_large: true,
            binary: false,
            size,
        }));
    }

    let bytes = {
        let p = abs.clone();
        with_sftp(&state, &conn, move |s| {
            let p = p.clone();
            async move { s.read(p).await }
        })
        .await
        .map_err(|e| RfsError(e.0, format!("Cannot read {}: {}", q.path, e.1)))?
    };

    // Same cheap sniff as the local side: a NUL byte in the first 8 KB.
    let binary = bytes.iter().take(8192).any(|&b| b == 0);
    Ok(Json(FileContent {
        path: q.path,
        content: if binary {
            String::new()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        },
        too_large: false,
        binary,
        size,
    }))
}

/// Every file in the workspace, for quick-open. `find` is one round trip;
/// walking the tree over SFTP would be hundreds.
async fn files(
    Path(conn): Path<String>,
    Query(q): Query<RootQuery>,
    State(state): State<Arc<AppState>>,
) -> RfsResult<Vec<String>> {
    let limit = q.limit.unwrap_or(MAX_FILE_LIST).min(MAX_FILE_LIST);
    let prunes: String = HARD_IGNORE
        .iter()
        .map(|d| format!("-name {} -prune -o ", shq(d)))
        .collect();
    let cmd = format!(
        "find {} {}-type f -print 2>/dev/null | head -n {}",
        shq(q.root.trim_end_matches('/')),
        prunes,
        limit
    );

    let out = exec(&state, &conn, cmd).await?;
    Ok(Json(
        out.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| rel_of(&q.root, l.trim_end_matches('\r')))
            .collect(),
    ))
}

/// Write a file in place, creating parent directories if needed.
async fn save(
    Path(conn): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SaveBody>,
) -> RfsResult<serde_json::Value> {
    let abs = join_remote(&body.root, &body.path)?;
    let parent = parent_of(&abs);
    let bytes = body.content.into_bytes();

    {
        let abs = abs.clone();
        let parent = parent.clone();
        let bytes = bytes.clone();
        with_sftp(&state, &conn, move |s| {
            let abs = abs.clone();
            let parent = parent.clone();
            let bytes = bytes.clone();
            async move {
                if let Some(p) = parent {
                    mkdir_p(&s, &p).await?;
                }
                write_whole(&s, &abs, &bytes).await
            }
        })
        .await
        .map_err(|e| RfsError(e.0, format!("Cannot save {}: {}", body.path, e.1)))?;
    }

    crate::logs::info(
        &state.log_store,
        "rfs",
        "save",
        conn_label(&state, &conn).await,
        format!("Wrote {} ({} bytes)", abs, bytes.len()),
    );
    Ok(Json(json!({ "success": true })))
}

async fn create(
    Path(conn): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> RfsResult<serde_json::Value> {
    let abs = join_remote(&body.root, &body.path)?;
    if abs == body.root.trim_end_matches('/') {
        return Err(RfsError::bad("Refusing to create the workspace root"));
    }
    let is_dir = body.dir;
    let parent = parent_of(&abs);

    {
        let abs = abs.clone();
        let parent = parent.clone();
        with_sftp(&state, &conn, move |s| {
            let abs = abs.clone();
            let parent = parent.clone();
            async move {
                if is_dir {
                    mkdir_p(&s, &abs).await
                } else {
                    if let Some(p) = parent {
                        mkdir_p(&s, &p).await?;
                    }
                    write_whole(&s, &abs, b"").await
                }
            }
        })
        .await
        .map_err(|e| RfsError(e.0, format!("Cannot create {}: {}", body.path, e.1)))?;
    }

    crate::logs::info(
        &state.log_store,
        "rfs",
        "create",
        conn_label(&state, &conn).await,
        format!("Created {} {}", if is_dir { "dir" } else { "file" }, abs),
    );
    Ok(Json(json!({ "success": true })))
}

async fn rename(
    Path(conn): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RenameBody>,
) -> RfsResult<serde_json::Value> {
    let from = join_remote(&body.root, &body.from)?;
    let to = join_remote(&body.root, &body.to)?;
    let root = body.root.trim_end_matches('/');
    if from == root || to == root {
        return Err(RfsError::bad("Refusing to rename the workspace root"));
    }

    {
        let (from, to) = (from.clone(), to.clone());
        with_sftp(&state, &conn, move |s| {
            let (from, to) = (from.clone(), to.clone());
            async move {
                if let Some(p) = parent_of(&to) {
                    mkdir_p(&s, &p).await?;
                }
                s.rename(from, to).await
            }
        })
        .await
        .map_err(|e| {
            RfsError(
                e.0,
                format!("Cannot rename {} → {}: {}", body.from, body.to, e.1),
            )
        })?;
    }

    crate::logs::info(
        &state.log_store,
        "rfs",
        "rename",
        conn_label(&state, &conn).await,
        format!("Renamed {} → {}", from, to),
    );
    Ok(Json(json!({ "success": true })))
}

async fn remove(
    Path(conn): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeleteBody>,
) -> RfsResult<serde_json::Value> {
    let abs = join_remote(&body.root, &body.path)?;
    // join_remote already refuses to climb out; this refuses the root itself,
    // which is the one path inside the workspace that must never be deleted.
    if abs == body.root.trim_end_matches('/') || body.path.trim().is_empty() {
        return Err(RfsError::bad("Refusing to delete the workspace root"));
    }

    let meta = {
        let p = abs.clone();
        with_sftp(&state, &conn, move |s| {
            let p = p.clone();
            async move { s.metadata(p).await }
        })
        .await
        .map_err(|e| RfsError(e.0, format!("Cannot stat {}: {}", body.path, e.1)))?
    };

    if meta.is_dir() {
        // SFTP has no recursive remove and walking a big tree over it is
        // hundreds of round trips. The path is already confined to the
        // workspace and cannot be the root.
        let out = exec(&state, &conn, format!("rm -rf -- {}", shq(&abs))).await?;
        if !out.trim().is_empty() {
            return Err(RfsError::upstream(format!(
                "Cannot delete {}: {}",
                body.path,
                out.trim()
            )));
        }
    } else {
        let p = abs.clone();
        with_sftp(&state, &conn, move |s| {
            let p = p.clone();
            async move { s.remove_file(p).await }
        })
        .await
        .map_err(|e| RfsError(e.0, format!("Cannot delete {}: {}", body.path, e.1)))?;
    }

    crate::logs::warn(
        &state.log_store,
        "rfs",
        "delete",
        conn_label(&state, &conn).await,
        format!("Deleted {}", abs),
    );
    Ok(Json(json!({ "success": true })))
}

/// Case-insensitive fixed-string search, run by the server's own grep.
async fn search(
    Path(conn): Path<String>,
    Query(q): Query<SearchQuery>,
    State(state): State<Arc<AppState>>,
) -> RfsResult<Vec<SearchHit>> {
    let needle = q.q.trim();
    if needle.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let limit = q.limit.unwrap_or(MAX_SEARCH_MATCHES).min(MAX_SEARCH_MATCHES);
    let excludes: String = HARD_IGNORE
        .iter()
        .map(|d| format!("--exclude-dir={} ", shq(d)))
        .collect();

    let cmd = format!(
        "grep -rniI --binary-files=without-match {}-F -e {} -- {} 2>/dev/null | head -n {}",
        excludes,
        shq(needle),
        shq(q.root.trim_end_matches('/')),
        limit
    );

    let out = exec(&state, &conn, cmd).await?;
    let mut hits = Vec::new();
    for raw in out.lines() {
        // path:line:text — the path itself may contain colons, so split from
        // the left twice only.
        let line = raw.trim_end_matches('\r');
        let Some((path, rest)) = line.split_once(':') else {
            continue;
        };
        let Some((no, text)) = rest.split_once(':') else {
            continue;
        };
        let Ok(no) = no.parse::<u32>() else { continue };
        hits.push(SearchHit {
            path: rel_of(&q.root, path),
            line: no,
            text: text.chars().take(400).collect(),
        });
    }
    Ok(Json(hits))
}

/// `git status --porcelain`, in the same `{files: {path: "XY"}}` shape the
/// local workspace serves.
async fn git_status(
    Path(conn): Path<String>,
    Query(q): Query<RootQuery>,
    State(state): State<Arc<AppState>>,
) -> RfsResult<serde_json::Value> {
    let cmd = format!(
        "git -C {} status --porcelain=v1 -z 2>/dev/null",
        shq(q.root.trim_end_matches('/'))
    );
    let out = exec(&state, &conn, cmd).await.unwrap_or_default();

    let mut map = serde_json::Map::new();
    for entry in out.split('\0').filter(|e| e.len() > 3) {
        let code = entry[..2].to_string();
        let path = entry[3..].to_string();
        map.insert(path, json!(code));
    }
    Ok(Json(json!({ "files": map })))
}
