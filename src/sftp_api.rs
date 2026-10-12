use crate::api::AppState;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::{get, post},
};
use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

#[derive(Serialize)]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_time: i64,
}

#[derive(Deserialize)]
pub struct PathQuery {
    pub path: String,
}

#[derive(Deserialize)]
pub struct TransferRequest {
    pub source: String,
    pub target: String,
}

#[derive(Deserialize)]
pub struct SftpConnectRequest {
    pub host_id: String,
}

#[derive(Serialize)]
pub struct SftpConnectResponse {
    pub conn_id: String,
}

/// What a folder upload actually moved. A single unreadable file should not
/// strand the rest of the tree, so the walk keeps going and reports what
/// failed instead of aborting on the first error.
#[derive(Serialize)]
pub struct UploadDirResponse {
    pub dirs: usize,
    pub files: usize,
    pub bytes: u64,
    pub failed: usize,
    pub errors: Vec<String>,
}

impl UploadDirResponse {
    /// Keep the first handful of messages: a folder with a thousand
    /// unreadable files should not answer with a thousand lines.
    fn fail(&mut self, message: String) {
        self.failed += 1;
        if self.errors.len() < 20 {
            self.errors.push(message);
        }
    }
}

pub fn sftp_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/local/ls", get(local_ls))
        .route("/remote/:conn_id/ls", get(remote_ls))
        .route("/remote/:conn_id/download", post(download_file))
        .route("/remote/:conn_id/upload", post(upload_file))
        .route("/remote/:conn_id/upload-dir", post(upload_dir))
        .route("/connect", post(connect_sftp))
}

async fn local_ls(
    Query(query): Query<PathQuery>,
) -> axum::response::Result<Json<Vec<FileNode>>, axum::http::StatusCode> {
    let mut entries = match fs::read_dir(&query.path).await {
        Ok(e) => e,
        Err(_) => return Err(axum::http::StatusCode::NOT_FOUND),
    };

    let mut files = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(metadata) = entry.metadata().await {
            let modified = metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            files.push(FileNode {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry.path().to_string_lossy().to_string(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified_time: modified,
            });
        }
    }

    // Sort directories first, then alphabetical
    files.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));

    Ok(Json(files))
}

async fn remote_ls(
    Path(conn_id): Path<String>,
    Query(query): Query<PathQuery>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Result<Json<Vec<FileNode>>, axum::http::StatusCode> {
    if let Some(client_arc) = state.connections.get(&conn_id).await {
        let mut client = client_arc.lock().await;

        let path = if query.path.is_empty() || query.path == "." {
            String::from(".") // Get default remote dir if empty
        } else {
            query.path.clone()
        };

        if let Ok(mut sftp) = client.get_sftp().await {
            let mut files = Vec::new();
            if let Ok(entries) = sftp.read_dir(path.clone()).await {
                for entry in entries {
                    let name_str = entry.file_name();
                    if name_str == "." || name_str == ".." {
                        continue;
                    }

                    let is_dir = entry.metadata().is_dir();
                    let size = entry.metadata().len();
                    let modified_time = entry
                        .metadata()
                        .modified()
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;

                    let full_path = entry.path();

                    files.push(FileNode {
                        name: name_str,
                        path: full_path,
                        is_dir,
                        size,
                        modified_time,
                    });
                }
            }
            files.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
            Ok(Json(files))
        } else {
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

async fn download_file(
    Path(conn_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<TransferRequest>,
) -> axum::response::Result<Json<bool>, axum::http::StatusCode> {
    if let Some(client_arc) = state.connections.get(&conn_id).await {
        let mut client = client_arc.lock().await;
        if let Ok(mut sftp) = client.get_sftp().await {
            let mut file = match sftp.open(payload.source.clone()).await {
                Ok(f) => f,
                Err(_) => return Err(axum::http::StatusCode::NOT_FOUND),
            };

            let mut contents = Vec::new();
            if file.read_to_end(&mut contents).await.is_err() {
                return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            }

            if fs::write(&payload.target, contents).await.is_err() {
                return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            }

            Ok(Json(true))
        } else {
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

async fn upload_file(
    Path(conn_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<TransferRequest>,
) -> axum::response::Result<Json<bool>, axum::http::StatusCode> {
    if let Some(client_arc) = state.connections.get(&conn_id).await {
        let contents = match fs::read(&payload.source).await {
            Ok(c) => c,
            Err(_) => return Err(axum::http::StatusCode::NOT_FOUND),
        };

        let mut client = client_arc.lock().await;
        if let Ok(mut sftp) = client.get_sftp().await {
            let mut file = match sftp.create(payload.target.clone()).await {
                Ok(f) => f,
                Err(_) => return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
            };

            if file.write_all(&contents).await.is_err() {
                return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            }

            // Writes are pipelined and only settled when the handle closes;
            // dropping the file here would risk dropping its tail with it.
            if file.shutdown().await.is_err() {
                return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            }

            Ok(Json(true))
        } else {
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    } else {
        Err(axum::http::StatusCode::NOT_FOUND)
    }
}

/// Upload a whole local folder, `target` being the remote path the folder
/// should land at — `scp -r` for the right-click menu in the file browser.
async fn upload_dir(
    Path(conn_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<TransferRequest>,
) -> Result<Json<UploadDirResponse>, (axum::http::StatusCode, String)> {
    let client_arc = state.connections.get(&conn_id).await.ok_or((
        axum::http::StatusCode::NOT_FOUND,
        "SFTP connection is gone".to_string(),
    ))?;

    if !fs::metadata(&payload.source)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("{} is not a folder", payload.source),
        ));
    }

    // The SFTP session rides its own channel, so the connection lock is only
    // needed to open it. Holding it for the whole walk would freeze the
    // directory listings the user is browsing while a big folder goes up.
    let sftp = {
        let mut client = client_arc.lock().await;
        client.get_sftp().await.map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("SFTP session failed: {e}"),
            )
        })?
    };

    let mut report = UploadDirResponse {
        dirs: 0,
        files: 0,
        bytes: 0,
        failed: 0,
        errors: Vec::new(),
    };

    ensure_remote_dir(&sftp, &payload.target)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    report.dirs += 1;

    // Depth-first with an explicit stack: recursing across an await point
    // would need boxing for nothing. Symlinked directories are copied as
    // files rather than walked, so a link loop cannot spin this forever.
    let mut pending = vec![(PathBuf::from(&payload.source), payload.target.clone())];
    while let Some((local_dir, remote_dir)) = pending.pop() {
        let mut entries = match fs::read_dir(&local_dir).await {
            Ok(e) => e,
            Err(e) => {
                report.fail(format!("read {}: {e}", local_dir.display()));
                continue;
            }
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let remote_path = join_remote(&remote_dir, &name);
            let is_dir = entry
                .file_type()
                .await
                .map(|t| t.is_dir())
                .unwrap_or(false);

            if is_dir {
                match ensure_remote_dir(&sftp, &remote_path).await {
                    Ok(()) => {
                        report.dirs += 1;
                        pending.push((entry.path(), remote_path));
                    }
                    Err(e) => report.fail(e),
                }
            } else {
                match upload_one(&sftp, &entry.path(), &remote_path).await {
                    Ok(bytes) => {
                        report.files += 1;
                        report.bytes += bytes;
                    }
                    Err(e) => report.fail(e),
                }
            }
        }
    }

    Ok(Json(report))
}

async fn connect_sftp(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SftpConnectRequest>,
) -> axum::response::Result<Json<SftpConnectResponse>, axum::http::StatusCode> {
    let host = match state
        .hosts
        .get_all()
        .into_iter()
        .find(|h| h.id == payload.host_id)
    {
        Some(h) => h,
        None => return Err(axum::http::StatusCode::NOT_FOUND),
    };

    let mut password = host.password.clone();
    let mut key_pair = None;

    if let Some(kid) = &host.keychain_id {
        if let Some(kitem) = state.keychain.get(kid) {
            match kitem.item_type {
                crate::keychain::KeychainItemType::Password => {
                    password = Some(kitem.value);
                }
                crate::keychain::KeychainItemType::PrivateKey => {
                    if let Ok(kp) =
                        russh_keys::decode_secret_key(kitem.value.as_str(), password.as_deref())
                    {
                        key_pair = Some(kp);
                    }
                }
            }
        }
    }

    match crate::client::SshClient::connect(
        &host.host,
        host.port,
        &host.user,
        password.as_deref(),
        key_pair,
        None,
    )
    .await
    {
        Ok(client) => {
            let conn_id = state.connections.add(client).await;
            Ok(Json(SftpConnectResponse { conn_id }))
        }
        Err(_) => Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Append a name to a remote directory path. The browser starts at `.`, the
/// login directory, where a bare name is already the path the server wants.
fn join_remote(base: &str, name: &str) -> String {
    if base.is_empty() || base == "." {
        name.to_string()
    } else if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Create a remote directory unless it is already there: `create_dir` fails on
/// an existing path, and uploading into a folder that exists is the normal
/// case for a second run.
async fn ensure_remote_dir(sftp: &SftpSession, path: &str) -> Result<(), String> {
    if sftp.try_exists(path.to_string()).await.unwrap_or(false) {
        return Ok(());
    }

    sftp.create_dir(path.to_string())
        .await
        .map_err(|e| format!("mkdir {path}: {e}"))
}

/// Stream one file across instead of reading it into memory first: a folder
/// upload is exactly where the multi-gigabyte file turns up.
async fn upload_one(sftp: &SftpSession, local: &FsPath, remote: &str) -> Result<u64, String> {
    let mut source = fs::File::open(local)
        .await
        .map_err(|e| format!("read {}: {e}", local.display()))?;

    let mut target = sftp
        .create(remote.to_string())
        .await
        .map_err(|e| format!("create {remote}: {e}"))?;

    let copied = tokio::io::copy(&mut source, &mut target)
        .await
        .map_err(|e| format!("write {remote}: {e}"))?;

    // Writes are pipelined and only settled when the handle closes.
    target
        .shutdown()
        .await
        .map_err(|e| format!("close {remote}: {e}"))?;

    Ok(copied)
}
