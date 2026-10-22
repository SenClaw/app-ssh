mod api;
mod client;
pub mod connection;
mod keychain;
mod logs;
mod mcp;
mod models;
pub mod rfs;
mod security;
mod settings;
mod snippets;
mod sftp_api;
mod sync;

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let port = std::env::var("PORT").unwrap_or_else(|_| "8081".to_string());

    let api_router = api::api_router();

    // Locate web/dist directory
    let exe_dir = std::env::current_exe()
        .map(|p| p.parent().unwrap().to_path_buf())
        .unwrap_or_default();

    let mut dist_path = std::path::PathBuf::from("web/dist");
    if std::path::Path::new("web_dist").exists() {
        dist_path = std::path::PathBuf::from("web_dist");
    } else if std::path::Path::new("apps/ssh-manager/web/dist").exists() {
        dist_path = std::path::PathBuf::from("apps/ssh-manager/web/dist");
    } else if exe_dir.join("web_dist").exists() {
        dist_path = exe_dir.join("web_dist");
    } else if exe_dir.join("web").join("dist").exists() {
        dist_path = exe_dir.join("web").join("dist");
    }

    let serve_dir = ServeDir::new(&dist_path).not_found_service(
        tower_http::services::ServeFile::new(dist_path.join("index.html")),
    );

    let app = Router::new()
        .nest("/api", api_router)
        .fallback_service(serve_dir)
        .layer(CorsLayer::permissive());

    // Loopback by default. A Space App authenticates nothing of its own — the
    // daemon reaches it over 127.0.0.1 and the UI is same-origin — so binding
    // 0.0.0.0 hands the whole REST + MCP surface to anyone on the LAN. Set
    // SENCLAW_BIND_HOST=0.0.0.0 to opt in to that explicitly.
    let host = std::env::var("SENCLAW_BIND_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}"))
        .await
        .unwrap();
    println!("SSH Manager running on http://{host}:{port}");

    axum::serve(listener, app).await.unwrap();
}
pub mod port_forwarding;
pub mod test_sftp;
