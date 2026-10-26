use axum::{
    Json,
    extract::State,
    response::sse::{Event, Sse},
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc};

use crate::api::AppState;

#[derive(Deserialize, Debug)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

/// Actionable error for the connect tools / `ssh_execute_command` when the
/// connection parameters can't be resolved. The old message ("Missing host,
/// port, or user") gave the model nothing to correct, so it would blindly
/// re-issue the same failing call and spin. This spells out exactly what to
/// pass — and, when a `host_id` was supplied but didn't match, says so instead
/// of pretending the parameters were simply absent.
pub(crate) fn missing_connection_params_error(args: &Value, state: &AppState) -> String {
    let hosts = state.hosts.get_all();
    let known: Vec<String> = hosts
        .iter()
        .map(|h| {
            format!(
                "- id={} (name: {}, {}@{}:{})",
                h.id, h.name, h.user, h.host, h.port
            )
        })
        .collect();

    if let Some(id) = args["host_id"].as_str() {
        if !id.is_empty() && !hosts.iter().any(|h| h.id == id) {
            return if known.is_empty() {
                format!(
                    "host_id '{id}' was not found and no hosts are saved. Add one with ssh_add_host, \
                     or pass explicit host, port and user — do not retry the same host_id."
                )
            } else {
                format!(
                    "host_id '{id}' was not found. Call ssh_list_hosts and use the exact `id` of the \
                     target server (do not retry the same value). Known hosts:\n{}",
                    known.join("\n")
                )
            };
        }
    }

    if known.is_empty() {
        "Missing connection parameters. For a saved host use ssh_start_connect_id with `host_id`; \
         otherwise pass `host` and `user` (optional `port`, default 22) to ssh_start_connect. \
         No hosts are currently saved — add one with ssh_add_host first."
            .to_string()
    } else {
        format!(
            "Missing connection parameters. For a saved host call ssh_start_connect_id with `host_id` \
             (the exact `id` from ssh_list_hosts). For an unsaved server call ssh_start_connect with \
             `host` and `user` (optional `port`, default 22, and `password`) — not an empty call. \
             Saved hosts:\n{}",
            known.join("\n")
        )
    }
}

/// How the caller referred to a port forwarding rule — for the log line.
fn port_forward_ref(args: &Value) -> String {
    args["rule_id"]
        .as_str()
        .or_else(|| args["name"].as_str())
        .unwrap_or("?")
        .to_string()
}

/// "Which rule did you mean?" — spelled out with every rule's exact `id`, so
/// the model can correct the call instead of retrying the same wrong value.
fn port_forward_not_found_error(
    rules: &[crate::models::PortForwardingRule],
    asked: Option<&str>,
) -> String {
    if rules.is_empty() {
        return "No port forwarding rules exist yet — create one with ssh_add_port_forward."
            .to_string();
    }
    let known: Vec<String> = rules
        .iter()
        .map(|r| {
            format!(
                "- id={} (name: {}, {}:{} → {}:{})",
                r.id, r.name, r.bind_address, r.local_port, r.destination_address, r.destination_port
            )
        })
        .collect();
    match asked {
        Some(a) => format!(
            "No port forwarding rule matches '{}'. Use the exact `id` from ssh_list_port_forwards \
             (do not retry the same value). Known rules:\n{}",
            a,
            known.join("\n")
        ),
        None => format!(
            "Pass `rule_id` (the exact `id` from ssh_list_port_forwards) or `name` to say which \
             rule. Known rules:\n{}",
            known.join("\n")
        ),
    }
}

/// Resolve `rule_id` / `name` to a stored rule. An id wins; a name is accepted
/// too, because that is what a model reaches for when the user said "the k8s
/// tunnel" — but only when it is unambiguous.
fn resolve_port_forward(
    state: &AppState,
    args: &Value,
) -> Result<crate::models::PortForwardingRule, String> {
    let rules = state.port_forwarding_store.get_all();
    let asked = args["rule_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| args["name"].as_str().filter(|s| !s.is_empty()));

    let Some(asked) = asked else {
        return Err(port_forward_not_found_error(&rules, None));
    };

    if let Some(r) = rules.iter().find(|r| r.id == asked) {
        return Ok(r.clone());
    }

    let by_name: Vec<&crate::models::PortForwardingRule> = rules
        .iter()
        .filter(|r| r.name.eq_ignore_ascii_case(asked))
        .collect();
    match by_name.len() {
        1 => Ok(by_name[0].clone()),
        0 => Err(port_forward_not_found_error(&rules, Some(asked))),
        n => Err(format!(
            "{} rules are named '{}' — pass the exact `rule_id` instead:\n{}",
            n,
            asked,
            by_name
                .iter()
                .map(|r| format!(
                    "- id={} ({}:{} → {}:{})",
                    r.id, r.bind_address, r.local_port, r.destination_address, r.destination_port
                ))
                .collect::<Vec<_>>()
                .join("\n")
        )),
    }
}

/// One reply shape for the port forwarding tools.
fn tool_reply(id: Option<Value>, text: String, is_error: bool) -> Value {
    let mut result = json!({ "content": [{ "type": "text", "text": text }] });
    if is_error {
        result["isError"] = json!(true);
    }
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub async fn mcp_sse(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.mcp_tx.subscribe();

    let stream = async_stream::stream! {
        // First message: send the endpoint
        let endpoint_msg = "/api/mcp/message".to_string();
        yield Ok(Event::default().event("endpoint").data(endpoint_msg));

        while let Ok(msg) = rx.recv().await {
            yield Ok(Event::default().event("message").data(msg));
        }
    };

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

pub async fn mcp_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JsonRpcRequest>,
) -> Json<Value> {
    if req.method == "initialize" {
        let resp = json!({
            "jsonrpc": "2.0",
            "id": req.id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "ssh-manager-mcp",
                    "version": "1.0.0"
                }
            }
        });
        let _ = state.mcp_tx.send(resp.to_string());
        return Json(resp);
    }

    if req.method == "ping" {
        let resp = json!({
            "jsonrpc": "2.0",
            "id": req.id,
            "result": {}
        });
        let _ = state.mcp_tx.send(resp.to_string());
        return Json(resp);
    }

    if req.method == "notifications/initialized" {
        // Nothing to do for initialized notification
        return Json(json!({
            "jsonrpc": "2.0",
            "id": req.id,
            "result": {}
        }));
    }

    if req.method == "tools/list" {
        let tools = json!({
            "tools": [
                {
                    "name": "ssh_list_hosts",
                    "description": "List all available SSH hosts managed by SSH Manager.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "ssh_add_host",
                    "description": "Add a new SSH host to the manager.",
                    "inputSchema": { "type": "object", "properties": { "id": {"type": "string"}, "host": {"type": "string"}, "port": {"type": "number"}, "user": {"type": "string"}, "password": {"type": "string"} }, "required": ["id", "host", "port", "user"] }
                },
                {
                    "name": "ssh_update_host",
                    "description": "Update an existing SSH host.",
                    "inputSchema": { "type": "object", "properties": { "id": {"type": "string"}, "host": {"type": "string"}, "port": {"type": "number"}, "user": {"type": "string"}, "password": {"type": "string"} }, "required": ["id", "host", "port", "user"] }
                },
                {
                    "name": "ssh_remove_host",
                    "description": "Remove an SSH host from the manager.",
                    "inputSchema": { "type": "object", "properties": { "id": {"type": "string"} }, "required": ["id"] }
                },
                {
                    "name": "ssh_start_connect_id",
                    "description": "Open a stateful SSH connection to a SAVED host. Pass `host_id` = the exact `id` field returned by ssh_list_hosts (NOT the host name or IP). Credentials come from the saved host entry. Returns a `connection_id` to use with ssh_execute_command / sftp_* tools. Prefer this tool whenever the server already exists in ssh_list_hosts.",
                    "inputSchema": { "type": "object", "properties": { "host_id": {"type": "string", "description": "Exact `id` of a saved host from ssh_list_hosts"} }, "required": ["host_id"] }
                },
                {
                    "name": "ssh_start_connect",
                    "description": "Open a stateful SSH connection using EXPLICIT connection details: `host` (IP or hostname) and `user` are required; `port` defaults to 22; `password` is optional. Use this ONLY for servers that are not saved in ssh_list_hosts — for saved hosts use ssh_start_connect_id instead. Returns a `connection_id` to use with ssh_execute_command / sftp_* tools.",
                    "inputSchema": { "type": "object", "properties": { "host": {"type": "string", "description": "Server IP address or hostname"}, "port": {"type": "number", "description": "SSH port, default 22"}, "user": {"type": "string", "description": "SSH username, e.g. root"}, "password": {"type": "string", "description": "Password (omit to use key/agent auth)"} }, "required": ["host", "user"] }
                },
                {
                    "name": "ssh_close_connect",
                    "description": "Close an active SSH connection.",
                    "inputSchema": { "type": "object", "properties": { "connection_id": {"type": "string"} }, "required": ["connection_id"] }
                },
                {
                    "name": "ssh_check_connect",
                    "description": "Check if an SSH connection is active.",
                    "inputSchema": { "type": "object", "properties": { "connection_id": {"type": "string"} }, "required": ["connection_id"] }
                },
                {
                    "name": "ssh_list_connected",
                    "description": "List all active SSH connection IDs.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "ssh_execute_command",
                    "description": "Execute a shell command on a specific SSH host or active connection_id. Commands are checked first: read-only ones run at once, risky ones (delete, restart, install, change permissions, read secrets, ...) are NOT run but wait for the admin's approval — the reply then starts with 'NOT RUN YET' and gives an approval_id to pass to ssh_command_status. Never reword a command to get around the check.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "connection_id": { "type": "string" },
                            "host_id": { "type": "string" },
                            "host": { "type": "string" },
                            "port": { "type": "number" },
                            "user": { "type": "string" },
                            "password": { "type": "string" },
                            "command": { "type": "string" }
                        },
                        "required": ["command"]
                    }
                },
                {
                    "name": "ssh_command_status",
                    "description": "Status of a command that is waiting for the admin's approval (the approval_id from ssh_execute_command). Returns JSON: status is pending, running, done, failed, rejected or expired; once done, `output` holds what the command printed. To wait, arm schedule_watch on this tool until status is done, failed, rejected or expired instead of calling it in a loop.",
                    "inputSchema": { "type": "object", "properties": { "approval_id": { "type": "string", "description": "approval_id from ssh_execute_command" } }, "required": ["approval_id"] }
                },
                {
                    "name": "sftp_list_directory",
                    "description": "List contents of a directory over SFTP.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "connection_id": { "type": "string" },
                            "path": { "type": "string" }
                        },
                        "required": ["connection_id", "path"]
                    }
                },
                {
                    "name": "sftp_read_file",
                    "description": "Read file contents over SFTP.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "connection_id": { "type": "string" },
                            "path": { "type": "string" }
                        },
                        "required": ["connection_id", "path"]
                    }
                },
                {
                    "name": "sftp_write_file",
                    "description": "Write string content to a file over SFTP.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "connection_id": { "type": "string" },
                            "path": { "type": "string" },
                            "content": { "type": "string" }
                        },
                        "required": ["connection_id", "path", "content"]
                    }
                },
                {
                    "name": "ssh_list_port_forwards",
                    "description": "List every local port forwarding rule (ssh -L) with its LIVE status. `status` is \"active\" only while the tunnel is really forwarding: \"error\" (with `last_error`) means it is not, and \"inactive\" means it is switched off. Also returns `listening_on`, `uptime_seconds`, `connections` (open now) and `total_connections`. Call this before starting or stopping a tunnel to get the exact `id`.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "ssh_start_port_forward",
                    "description": "Open (start) a port forwarding tunnel. Identify the rule with `rule_id` (the exact `id` from ssh_list_port_forwards) or `name`. The local port is bound and the SSH connection is verified before this returns, so success means the tunnel is genuinely forwarding; failure explains why (port already in use, host unreachable, auth rejected).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "rule_id": { "type": "string", "description": "Exact `id` of a rule from ssh_list_port_forwards" },
                            "name": { "type": "string", "description": "Rule name, if the id is not known" }
                        }
                    }
                },
                {
                    "name": "ssh_stop_port_forward",
                    "description": "Close (stop) a port forwarding tunnel and release the local port. Identify the rule with `rule_id` (preferred) or `name`. The rule itself is kept — it just stops being re-opened on restart until it is started again.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "rule_id": { "type": "string", "description": "Exact `id` of a rule from ssh_list_port_forwards" },
                            "name": { "type": "string", "description": "Rule name, if the id is not known" }
                        }
                    }
                },
                {
                    "name": "ssh_add_port_forward",
                    "description": "Create a local port forwarding rule: traffic to `bind_address`:`local_port` on this machine is forwarded through the saved SSH host `host_id` to `destination_address`:`destination_port` on the far side. `host_id` is the exact `id` from ssh_list_hosts. The tunnel is opened immediately unless `start` is false.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Label for the rule, e.g. 'k8s api'" },
                            "host_id": { "type": "string", "description": "Exact `id` of a saved host from ssh_list_hosts" },
                            "local_port": { "type": "number", "description": "Local port to listen on" },
                            "bind_address": { "type": "string", "description": "Local bind address, default 127.0.0.1" },
                            "destination_address": { "type": "string", "description": "Target address as seen from the SSH host, default 127.0.0.1" },
                            "destination_port": { "type": "number", "description": "Target port on the far side" },
                            "start": { "type": "boolean", "description": "Open the tunnel right away, default true" }
                        },
                        "required": ["name", "host_id", "local_port", "destination_port"]
                    }
                },
                {
                    "name": "ssh_remove_port_forward",
                    "description": "Delete a port forwarding rule, stopping its tunnel first. Identify it with `rule_id` (preferred) or `name`.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "rule_id": { "type": "string", "description": "Exact `id` of a rule from ssh_list_port_forwards" },
                            "name": { "type": "string", "description": "Rule name, if the id is not known" }
                        }
                    }
                }
            ]
        });
        let resp = json!({
            "jsonrpc": "2.0",
            "id": req.id,
            "result": tools
        });

        let _ = state.mcp_tx.send(resp.to_string());
        return Json(resp);
    }

    if req.method == "tools/call" {
        let params = req.params.unwrap_or_default();
        let name = params["name"].as_str().unwrap_or("");
        let args = &params["arguments"];

        // Resolve a friendly host label.
        // Priority: explicit host_id (look up name in HostStore) → host:port → connection_id (look up its host_id).
        let host_label: Option<String> = {
            let resolve_by_id = |id: &str| -> Option<String> {
                state
                    .hosts
                    .get_all()
                    .into_iter()
                    .find(|h| h.id == id)
                    .map(|h| format!("{} ({}@{}:{})", h.name, h.user, h.host, h.port))
            };

            if let Some(hid) = args["host_id"].as_str() {
                resolve_by_id(hid).or_else(|| Some(hid.to_string()))
            } else if let (Some(h), Some(u)) = (args["host"].as_str(), args["user"].as_str()) {
                let p = args["port"].as_u64().unwrap_or(22);
                Some(format!("{}@{}:{}", u, h, p))
            } else if let Some(h) = args["host"].as_str() {
                Some(h.to_string())
            } else if let Some(cid) = args["connection_id"].as_str() {
                // connection_id → look up host_id via the active client → resolve name
                let host_id_opt: Option<String> =
                    if let Some(client_arc) = state.connections.get(cid).await {
                        client_arc.lock().await.host_id.clone()
                    } else {
                        None
                    };
                host_id_opt
                    .and_then(|hid| resolve_by_id(&hid))
                    .or_else(|| Some(format!("conn:{}", &cid[..8.min(cid.len())])))
            } else {
                None
            }
        };

        // Build a tool-specific message body.
        let detail = match name {
            "ssh_execute_command" => {
                let cmd = args["command"].as_str().unwrap_or("");
                format!("exec: {}", cmd)
            }
            "ssh_command_status" => format!(
                "approval status {}",
                args["approval_id"].as_str().unwrap_or("?")
            ),
            "ssh_start_connect" => "open connection (explicit params)".to_string(),
            "ssh_start_connect_id" => format!(
                "open connection (host_id={})",
                args["host_id"].as_str().unwrap_or("?")
            ),
            "ssh_close_connect" => {
                let cid = args["connection_id"].as_str().unwrap_or("?");
                format!("close connection_id={}", cid)
            }
            "ssh_check_connect" => {
                let cid = args["connection_id"].as_str().unwrap_or("?");
                format!("check connection_id={}", cid)
            }
            "ssh_list_connected" => "list active connections".to_string(),
            "ssh_list_hosts" => "list saved hosts".to_string(),
            "ssh_add_host" => format!("add host: {}", args["name"].as_str().unwrap_or("?")),
            "ssh_update_host" => format!("update host: {}", args["name"].as_str().unwrap_or("?")),
            "ssh_remove_host" => format!("remove host id={}", args["id"].as_str().unwrap_or("?")),
            "ssh_list_port_forwards" => "list port forwarding rules".to_string(),
            "ssh_start_port_forward" => format!("start tunnel {}", port_forward_ref(args)),
            "ssh_stop_port_forward" => format!("stop tunnel {}", port_forward_ref(args)),
            "ssh_add_port_forward" => format!(
                "add tunnel {}: {}:{} → {}:{}",
                args["name"].as_str().unwrap_or("?"),
                args["bind_address"].as_str().unwrap_or("127.0.0.1"),
                args["local_port"].as_u64().unwrap_or(0),
                args["destination_address"].as_str().unwrap_or("127.0.0.1"),
                args["destination_port"].as_u64().unwrap_or(0)
            ),
            "ssh_remove_port_forward" => format!("remove tunnel {}", port_forward_ref(args)),
            "sftp_list_directory" => format!("ls {}", args["path"].as_str().unwrap_or("/")),
            "sftp_read_file" => format!("read {}", args["path"].as_str().unwrap_or("?")),
            "sftp_write_file" => {
                let bytes = args["content"].as_str().map(|s| s.len()).unwrap_or(0);
                format!(
                    "write {} ({} bytes)",
                    args["path"].as_str().unwrap_or("?"),
                    bytes
                )
            }
            _ => format!("call: {}", name),
        };

        crate::logs::info(&state.log_store, "mcp", name, host_label.clone(), detail);

        if name == "ssh_list_hosts" {
            let hosts = state.hosts.get_all();
            // Security: never expose secrets to the agent. Strip `password` and
            // `keychain_id`; surface only boolean flags so the agent can still
            // reason about which auth method a host uses without seeing it.
            let safe: Vec<serde_json::Value> = hosts
                .iter()
                .map(|h| {
                    json!({
                        "id": h.id,
                        "name": h.name,
                        "host": h.host,
                        "port": h.port,
                        "user": h.user,
                        "tags": h.tags,
                        "has_password": h.password.as_deref().map(|p| !p.is_empty()).unwrap_or(false),
                        "has_keychain": h.keychain_id.is_some(),
                    })
                })
                .collect();
            let result = json!({
                "content": [{ "type": "text", "text": serde_json::to_string(&safe).unwrap() }]
            });
            let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_add_host" {
            let args = &params["arguments"];
            let id = args["id"].as_str().unwrap_or("").to_string();
            let host = args["host"].as_str().unwrap_or("").to_string();
            let port = args["port"].as_u64().unwrap_or(22) as u16;
            let user = args["user"].as_str().unwrap_or("").to_string();
            let password = args["password"].as_str().map(|s| s.to_string());
            let name_field = args["name"].as_str().unwrap_or(&host).to_string();
            state.hosts.add(crate::models::Host {
                id,
                name: name_field,
                host,
                port,
                user,
                password,
                keychain_id: None,
                tags: vec![],
                extra: Default::default(),
            });
            let result = json!({ "content": [{ "type": "text", "text": "Host added successfully." }

] });
            let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_update_host" {
            let args = &params["arguments"];
            let id = args["id"].as_str().unwrap_or("").to_string();
            let host = args["host"].as_str().unwrap_or("").to_string();
            let port = args["port"].as_u64().unwrap_or(22) as u16;
            let user = args["user"].as_str().unwrap_or("").to_string();
            let password = args["password"].as_str().map(|s| s.to_string());
            let name_field = args["name"].as_str().unwrap_or(&host).to_string();
            if state
                .hosts
                .update(
                    &id,
                    crate::models::Host {
                        id: id.clone(),
                        name: name_field,
                        host,
                        port,
                        user,
                        password,
                        keychain_id: None,
                        tags: vec![],
                        extra: Default::default(),
                    },
                )
                .is_some()
            {
                let result = json!({ "content": [{ "type": "text", "text": "Host updated successfully." }

] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            } else {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Host not found." }

] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }

        if name == "ssh_remove_host" {
            let args = &params["arguments"];
            let id = args["id"].as_str().unwrap_or("").to_string();
            if state.hosts.delete(&id) {
                let result = json!({ "content": [{ "type": "text", "text": "Host removed successfully." }

] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            } else {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Host not found." }

] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }

        if name == "ssh_start_connect" || name == "ssh_start_connect_id" {
            let args = &params["arguments"];
            let mut host = args["host"].as_str().map(|s| s.to_string());
            let mut port = args["port"].as_u64().map(|n| n as u16);
            let mut user = args["user"].as_str().map(|s| s.to_string());
            let mut password = args["password"].as_str().map(|s| s.to_string());

            // `ssh_start_connect_id` requires a saved host; `ssh_start_connect`
            // still resolves `host_id` if one is passed (legacy callers).
            if let Some(host_id) = args["host_id"].as_str() {
                if let Some(saved) = state.hosts.get_all().into_iter().find(|h| h.id == host_id) {
                    if host.is_none() {
                        host = Some(saved.host.clone());
                    }
                    if port.is_none() {
                        port = Some(saved.port);
                    }
                    if user.is_none() {
                        user = Some(saved.user.clone());
                    }
                    if password.is_none() {
                        password = saved.password.clone();
                    }
                }
            }

            // Explicit-params path: port is optional, default 22.
            if port.is_none() && host.is_some() && user.is_some() {
                port = Some(22);
            }

            if host.is_none() || user.is_none() || port.is_none() {
                let text = missing_connection_params_error(args, &state);
                let result =
                    json!({ "isError": true, "content": [{ "type": "text", "text": text }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }

            match crate::client::SshClient::connect(
                &host.unwrap(),
                port.unwrap(),
                &user.unwrap(),
                password.as_deref(),
                None,
                args["host_id"].as_str().map(|s| s.to_string()),
            )
            .await
            {
                Ok(c) => {
                    let conn_id = state.connections.add(c).await;

                    if let Some(host_id) = args["host_id"].as_str() {
                        if let Some(saved) =
                            state.hosts.get_all().into_iter().find(|h| h.id == host_id)
                        {
                            let _ = state.ui_tx.send(
                                json!({
                                    "type": "mcp_connect",
                                    "host_id": host_id,
                                    "host": saved
                                })
                                .to_string(),
                            );
                        }
                    }

                    let result = json!({ "content": [{ "type": "text", "text": format!("Connected successfully. connection_id: {}", conn_id) }

] });
                    let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }
                Err(e) => {
                    let result = json!({ "isError": true, "content": [{ "type": "text", "text": format!("Error: {}", e) }

] });
                    let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }
            }
        }

        if name == "ssh_close_connect" {
            let args = &params["arguments"];
            let conn_id = args["connection_id"].as_str().unwrap_or("");
            state.rfs.forget(conn_id).await;
            if state.connections.remove(conn_id).await {
                let result = json!({ "content": [{ "type": "text", "text": "Connection closed." }

] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            } else {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Connection not found." }

] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }

        if name == "ssh_check_connect" {
            let args = &params["arguments"];
            let conn_id = args["connection_id"].as_str().unwrap_or("");
            let is_active = state.connections.get(conn_id).await.is_some();
            let result = json!({ "content": [{ "type": "text", "text": format!("Active: {}", is_active) }

] });
            let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_list_connected" {
            let list = state.connections.list().await;
            let result = json!({ "content": [{ "type": "text", "text": serde_json::to_string(&list).unwrap() }

] });
            let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_execute_command" {
            let command = args["command"].as_str().unwrap_or("").to_string();
            // Where it would run comes first: an approval the admin cannot act
            // on (a closed connection, missing credentials) helps nobody.
            let target = match crate::remote::Target::from_args(&state, args).await {
                Ok(t) => t,
                Err(text) => {
                    let resp = tool_reply(req.id, text, true);
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }
            };
            let target_label = host_label.clone().unwrap_or_else(|| "unknown host".to_string());
            let resp = match crate::guard::http::gate(&state, &command, target.clone(), &target_label, "mcp").await {
                crate::guard::http::Gate::Run => match crate::remote::run(&state, &target, &command).await {
                    Ok(output) => tool_reply(req.id, output, false),
                    Err(e) => tool_reply(req.id, e, true),
                },
                crate::guard::http::Gate::Wait(a) => {
                    tool_reply(req.id, crate::guard::http::pending_text(&a), false)
                }
                crate::guard::http::Gate::Refused(d) => tool_reply(
                    req.id,
                    format!("Command '{}' blocked by user settings: {}", command, d.why()),
                    true,
                ),
            };
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_command_status" {
            let id = args["approval_id"].as_str().unwrap_or("").trim();
            let resp = match state.guard.approvals.get(id) {
                Some(a) => tool_reply(
                    req.id,
                    serde_json::to_string_pretty(&crate::guard::http::status_json(&a))
                        .unwrap_or_default(),
                    false,
                ),
                None => tool_reply(
                    req.id,
                    format!(
                        "No approval request with id '{id}'. Requests live in memory only, so an SSH Manager restart forgets them; send the command again if it is still needed."
                    ),
                    true,
                ),
            };
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        // ── Port forwarding ─────────────────────────────────────────────────
        // Every reply carries the rule's *live* status, so the agent can tell
        // "asked for" from "actually forwarding" without a follow-up call.

        if name == "ssh_list_port_forwards" {
            let views = crate::port_forwarding::all_views(&state).await;
            let text = if views.is_empty() {
                "No port forwarding rules yet — create one with ssh_add_port_forward.".to_string()
            } else {
                serde_json::to_string_pretty(&views)
                    .unwrap_or_else(|e| format!("Error serialising rules: {}", e))
            };
            let resp = tool_reply(req.id, text, false);
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_start_port_forward" {
            let rule = match resolve_port_forward(&state, args) {
                Ok(r) => r,
                Err(e) => {
                    let resp = tool_reply(req.id, e, true);
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }
            };

            let outcome = state
                .port_forwarding_manager
                .start_tunnel(state.clone(), rule.clone())
                .await;
            if outcome.is_ok() {
                state.port_forwarding_store.set_active(&rule.id, true);
            }

            let current = state
                .port_forwarding_store
                .get(&rule.id)
                .unwrap_or_else(|| rule.clone());
            let view = crate::port_forwarding::view_of(&state, current).await;
            let detail = serde_json::to_string(&view).unwrap_or_default();

            let (text, is_error) = match outcome {
                Ok(_) => (
                    format!("Tunnel open — {}\n{}", view.summary(), detail),
                    false,
                ),
                Err(e) => (
                    format!(
                        "Could not open tunnel '{}': {}\n{}",
                        rule.name, e, detail
                    ),
                    true,
                ),
            };
            let resp = tool_reply(req.id, text, is_error);
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_stop_port_forward" {
            let rule = match resolve_port_forward(&state, args) {
                Ok(r) => r,
                Err(e) => {
                    let resp = tool_reply(req.id, e, true);
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }
            };

            let was_running = state.port_forwarding_manager.stop_tunnel(&rule.id).await;
            state.port_forwarding_store.set_active(&rule.id, false);
            state.port_forwarding_manager.forget(&rule.id).await;

            let current = state
                .port_forwarding_store
                .get(&rule.id)
                .unwrap_or_else(|| rule.clone());
            let view = crate::port_forwarding::view_of(&state, current).await;
            let text = if was_running {
                format!(
                    "Tunnel closed, local port released — {}",
                    view.summary()
                )
            } else {
                format!(
                    "Tunnel '{}' was not running; it stays switched off — {}",
                    rule.name,
                    view.summary()
                )
            };
            let resp = tool_reply(req.id, text, false);
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_add_port_forward" {
            let rule_name = args["name"].as_str().unwrap_or("").trim().to_string();
            let host_id = args["host_id"].as_str().unwrap_or("").to_string();
            let local_port = args["local_port"].as_u64().unwrap_or(0);
            let destination_port = args["destination_port"].as_u64().unwrap_or(0);
            let bind_address = args["bind_address"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("127.0.0.1")
                .to_string();
            let destination_address = args["destination_address"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("127.0.0.1")
                .to_string();
            let start = args["start"].as_bool().unwrap_or(true);

            let hosts = state.hosts.get_all();
            let complaint = if rule_name.is_empty() {
                Some("Pass `name` — a label for the rule, e.g. 'k8s api'.".to_string())
            } else if !hosts.iter().any(|h| h.id == host_id) {
                Some(if hosts.is_empty() {
                    "No SSH hosts are saved — add one with ssh_add_host before creating a tunnel."
                        .to_string()
                } else {
                    format!(
                        "host_id '{}' was not found. Use the exact `id` from ssh_list_hosts. \
                         Saved hosts:\n{}",
                        host_id,
                        hosts
                            .iter()
                            .map(|h| format!(
                                "- id={} (name: {}, {}@{}:{})",
                                h.id, h.name, h.user, h.host, h.port
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                })
            } else if !(1..=65535).contains(&local_port) {
                Some("`local_port` must be a port number between 1 and 65535.".to_string())
            } else if !(1..=65535).contains(&destination_port) {
                Some("`destination_port` must be a port number between 1 and 65535.".to_string())
            } else {
                None
            };

            if let Some(text) = complaint {
                let resp = tool_reply(req.id, text, true);
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }

            let created = state
                .port_forwarding_store
                .add(crate::models::PortForwardingRule {
                    id: String::new(),
                    name: rule_name,
                    local_port: local_port as u16,
                    bind_address,
                    host_id,
                    destination_address,
                    destination_port: destination_port as u16,
                    active: false,
                    extra: Default::default(),
                });

            let outcome = if start {
                Some(
                    state
                        .port_forwarding_manager
                        .start_tunnel(state.clone(), created.clone())
                        .await,
                )
            } else {
                None
            };
            if matches!(outcome, Some(Ok(_))) {
                state.port_forwarding_store.set_active(&created.id, true);
            }

            let current = state
                .port_forwarding_store
                .get(&created.id)
                .unwrap_or_else(|| created.clone());
            let view = crate::port_forwarding::view_of(&state, current).await;
            let detail = serde_json::to_string(&view).unwrap_or_default();

            let (text, is_error) = match outcome {
                None => (
                    format!(
                        "Rule created (id={}), left switched off — {}\n{}",
                        created.id,
                        view.summary(),
                        detail
                    ),
                    false,
                ),
                Some(Ok(_)) => (
                    format!(
                        "Rule created (id={}) and tunnel open — {}\n{}",
                        created.id,
                        view.summary(),
                        detail
                    ),
                    false,
                ),
                // The rule exists either way: say so, or the agent creates a
                // duplicate on its next attempt.
                Some(Err(e)) => (
                    format!(
                        "Rule created (id={}) but the tunnel could not open: {}\nFix the cause and \
                         call ssh_start_port_forward with this id — do not create it again.\n{}",
                        created.id, e, detail
                    ),
                    true,
                ),
            };
            let resp = tool_reply(req.id, text, is_error);
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_remove_port_forward" {
            let rule = match resolve_port_forward(&state, args) {
                Ok(r) => r,
                Err(e) => {
                    let resp = tool_reply(req.id, e, true);
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }
            };

            let was_running = state.port_forwarding_manager.stop_tunnel(&rule.id).await;
            state.port_forwarding_manager.forget(&rule.id).await;
            let deleted = state.port_forwarding_store.delete(&rule.id);

            let text = if deleted {
                format!(
                    "Rule '{}' deleted{}.",
                    rule.name,
                    if was_running {
                        " and its tunnel closed"
                    } else {
                        ""
                    }
                )
            } else {
                format!("Rule '{}' was already gone.", rule.name)
            };
            let resp = tool_reply(req.id, text, !deleted);
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }
    }
    Json(json!("ok"))
}
