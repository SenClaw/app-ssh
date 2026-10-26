//! Running one command on a server: on an open connection, or on a fresh one
//! made for that command.
//!
//! Both the agent's `ssh_execute_command` and a command the admin approved
//! later come through here, so an approved command runs exactly where the
//! agent asked for it.

use serde_json::{Value, json};

use crate::api::AppState;
use crate::client::SshClient;

#[derive(Clone, Debug)]
pub enum Target {
    /// A connection opened earlier with `ssh_start_connect(_id)`.
    Connection { id: String },
    /// Connect for this one command. The password is kept in memory only.
    Host {
        host: String,
        port: u16,
        user: String,
        password: Option<String>,
        host_id: Option<String>,
    },
}

impl Target {
    /// Read the target from `ssh_execute_command`'s arguments. `Err` is the
    /// message for the agent (unknown connection, missing parameters).
    pub async fn from_args(state: &AppState, args: &Value) -> Result<Target, String> {
        if let Some(conn_id) = args["connection_id"].as_str() {
            return if state.connections.get(conn_id).await.is_some() {
                Ok(Target::Connection { id: conn_id.to_string() })
            } else {
                Err("Connection not found.".to_string())
            };
        }
        let mut host = args["host"].as_str().map(|s| s.to_string());
        let mut port = args["port"].as_u64().map(|n| n as u16);
        let mut user = args["user"].as_str().map(|s| s.to_string());
        let mut password = args["password"].as_str().map(|s| s.to_string());
        let host_id = args["host_id"].as_str().map(|s| s.to_string());
        if let Some(hid) = &host_id {
            if let Some(saved) = state.hosts.get(hid) {
                host = host.or(Some(saved.host));
                port = port.or(Some(saved.port));
                user = user.or(Some(saved.user));
                password = password.or(saved.password);
            }
        }
        match (host, port, user) {
            (Some(host), Some(port), Some(user)) => Ok(Target::Host {
                host,
                port,
                user,
                password,
                host_id,
            }),
            _ => Err(crate::mcp::missing_connection_params_error(args, state)),
        }
    }
}

/// Run `command` on `target`. `Err` means it could not be run at all
/// (connection gone, login refused); a command that ran and failed is `Ok`
/// with whatever it printed, as before.
pub async fn run(state: &AppState, target: &Target, command: &str) -> Result<String, String> {
    match target {
        Target::Connection { id } => {
            let Some(client_arc) = state.connections.get(id).await else {
                return Err("Connection not found (it may have been closed).".to_string());
            };
            let mut client = client_arc.lock().await;
            let output = client
                .execute(command)
                .await
                .unwrap_or_else(|e| format!("Error executing: {}", e));
            let _ = state.ui_tx.send(
                json!({
                    "type": "mcp_execute",
                    "host_id": client.host_id,
                    "command": command,
                    "output": output
                })
                .to_string(),
            );
            Ok(output)
        }
        Target::Host {
            host,
            port,
            user,
            password,
            host_id,
        } => {
            let mut client = SshClient::connect(
                host,
                *port,
                user,
                password.as_deref(),
                None,
                host_id.clone(),
            )
            .await
            .map_err(|e| format!("Error: {}", e))?;
            Ok(client
                .execute(command)
                .await
                .unwrap_or_else(|e| format!("Error executing: {}", e)))
        }
    }
}
