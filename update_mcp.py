import re

with open("src/mcp.rs", "r") as f:
    content = f.read()

# We need to replace the tools/list JSON
new_tools = """
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
                    "name": "ssh_start_connect",
                    "description": "Start a stateful SSH connection using a saved Host ID, or explicit params. Returns a connection_id.",
                    "inputSchema": { "type": "object", "properties": { "host_id": {"type": "string"}, "host": {"type": "string"}, "port": {"type": "number"}, "user": {"type": "string"}, "password": {"type": "string"} } }
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
                    "description": "Execute a shell command on a specific SSH host or active connection_id.",
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
                }
            ]
        });
"""

# Replace tools/list
content = re.sub(r'let tools = json!\(\{\s*"tools": \[.*?\]\s*\}\);', new_tools, content, flags=re.DOTALL)

new_tools_call = r"""
        if name == "ssh_list_hosts" {
            let hosts = state.hosts.get_all();
            let result = json!({ "content": [{ "type": "text", "text": serde_json::to_string(&hosts).unwrap() }] });
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
            state.hosts.add(crate::models::Host { id, host, port, user, password });
            let result = json!({ "content": [{ "type": "text", "text": "Host added successfully." }] });
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
            if state.hosts.update(&id, crate::models::Host { id: id.clone(), host, port, user, password }).is_some() {
                let result = json!({ "content": [{ "type": "text", "text": "Host updated successfully." }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            } else {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Host not found." }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }

        if name == "ssh_remove_host" {
            let args = &params["arguments"];
            let id = args["id"].as_str().unwrap_or("").to_string();
            if state.hosts.delete(&id) {
                let result = json!({ "content": [{ "type": "text", "text": "Host removed successfully." }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            } else {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Host not found." }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }

        if name == "ssh_start_connect" {
            let args = &params["arguments"];
            let mut host = args["host"].as_str().map(|s| s.to_string());
            let mut port = args["port"].as_u64().map(|n| n as u16);
            let mut user = args["user"].as_str().map(|s| s.to_string());
            let mut password = args["password"].as_str().map(|s| s.to_string());
            
            if let Some(host_id) = args["host_id"].as_str() {
                if let Some(saved) = state.hosts.get_all().into_iter().find(|h| h.id == host_id) {
                    if host.is_none() { host = Some(saved.host.clone()); }
                    if port.is_none() { port = Some(saved.port); }
                    if user.is_none() { user = Some(saved.user.clone()); }
                    if password.is_none() { password = saved.password.clone(); }
                }
            }

            if host.is_none() || user.is_none() || port.is_none() {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Missing host, port, or user" }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }

            match crate::client::SshClient::connect(&host.unwrap(), port.unwrap(), &user.unwrap(), password.as_deref(), None).await {
                Ok(c) => {
                    let conn_id = state.connections.add(c).await;
                    let result = json!({ "content": [{ "type": "text", "text": format!("Connected successfully. connection_id: {}", conn_id) }] });
                    let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }
                Err(e) => {
                    let result = json!({ "isError": true, "content": [{ "type": "text", "text": format!("Error: {}", e) }] });
                    let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }
            }
        }

        if name == "ssh_close_connect" {
            let args = &params["arguments"];
            let conn_id = args["connection_id"].as_str().unwrap_or("");
            if state.connections.remove(conn_id).await {
                let result = json!({ "content": [{ "type": "text", "text": "Connection closed." }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            } else {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Connection not found." }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }

        if name == "ssh_check_connect" {
            let args = &params["arguments"];
            let conn_id = args["connection_id"].as_str().unwrap_or("");
            let is_active = state.connections.get(conn_id).await.is_some();
            let result = json!({ "content": [{ "type": "text", "text": format!("Active: {}", is_active) }] });
            let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_list_connected" {
            let list = state.connections.list().await;
            let result = json!({ "content": [{ "type": "text", "text": serde_json::to_string(&list).unwrap() }] });
            let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
            let _ = state.mcp_tx.send(resp.to_string());
            return Json(resp);
        }

        if name == "ssh_execute_command" {
            let args = &params["arguments"];
            let command = args["command"].as_str().unwrap_or("").to_string();
            
            if let Some(conn_id) = args["connection_id"].as_str() {
                if let Some(client_arc) = state.connections.get(conn_id).await {
                    let mut client = client_arc.lock().await;
                    let output = client.execute(&command).await.unwrap_or_else(|e| format!("Error executing: {}", e));
                    let result = json!({ "content": [{ "type": "text", "text": output }] });
                    let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                } else {
                    let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Connection not found." }] });
                    let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }
            } else {
                let mut host = args["host"].as_str().map(|s| s.to_string());
                let mut port = args["port"].as_u64().map(|n| n as u16);
                let mut user = args["user"].as_str().map(|s| s.to_string());
                let mut password = args["password"].as_str().map(|s| s.to_string());
                
                if let Some(host_id) = args["host_id"].as_str() {
                    if let Some(saved) = state.hosts.get_all().into_iter().find(|h| h.id == host_id) {
                        if host.is_none() { host = Some(saved.host.clone()); }
                        if port.is_none() { port = Some(saved.port); }
                        if user.is_none() { user = Some(saved.user.clone()); }
                        if password.is_none() { password = saved.password.clone(); }
                    }
                }

                if host.is_none() || user.is_none() || port.is_none() {
                    let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Missing host, port, or user" }] });
                    let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                    let _ = state.mcp_tx.send(resp.to_string());
                    return Json(resp);
                }

                let mut client = match crate::client::SshClient::connect(&host.unwrap(), port.unwrap(), &user.unwrap(), password.as_deref(), None).await {
                    Ok(c) => c,
                    Err(e) => {
                        let resp = json!({
                            "jsonrpc": "2.0", "id": req.id,
                            "result": { "isError": true, "content": [{ "type": "text", "text": format!("Error: {}", e) }] }
                        });
                        let _ = state.mcp_tx.send(resp.to_string());
                        return Json(resp);
                    }
                };
                let output = client.execute(&command).await.unwrap_or_else(|e| format!("Error executing: {}", e));
                let resp = json!({
                    "jsonrpc": "2.0", "id": req.id,
                    "result": { "content": [{ "type": "text", "text": output }] }
                });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }
"""

content = re.sub(r'if name == "ssh_list_hosts" \{.*\}', new_tools_call, content, flags=re.DOTALL)

with open("src/mcp.rs", "w") as f:
    f.write(content)
