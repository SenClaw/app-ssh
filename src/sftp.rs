use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::AppState;
use crate::client::SshClient;

#[derive(Deserialize)]
pub struct SftpListQuery {
    path: Option<String>,
}

#[derive(Serialize)]
pub struct SftpFile {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
}

#[derive(Serialize)]
pub struct SftpListResponse {
    pub files: Vec<SftpFile>,
    pub current_path: String,
}

pub async fn sftp_list(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<SftpListQuery>,
) -> Result<Json<SftpListResponse>, String> {
    let host = state.hosts.get(&id).ok_or("Host not found")?;
    let path = query.path.unwrap_or_else(|| ".".to_string());

    let mut client = SshClient::connect(
        &host.host,
        host.port,
        &host.user,
        host.password.as_deref(),
        None, // KeyPair support to be added
    )
    .await
    .map_err(|e| format!("Connection failed: {}", e))?;

    let files = client
        .sftp_list_dir(&path)
        .await
        .map_err(|e| format!("SFTP error: {}", e))?;

    Ok(Json(SftpListResponse {
        files,
        current_path: path,
    }))
}
