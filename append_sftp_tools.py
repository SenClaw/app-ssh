import re

with open("src/mcp.rs", "r") as f:
    content = f.read()

sftp_tools_json = """
                ,
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
                }
"""

content = re.sub(r'(\s*]\s*}\);\s*let resp = json!\()', sftp_tools_json + r'\1', content)

sftp_handlers = r"""
        if name == "sftp_list_directory" {
            let args = &params["arguments"];
            let conn_id = args["connection_id"].as_str().unwrap_or("");
            let path = args["path"].as_str().unwrap_or(".");

            if let Some(client_arc) = state.connections.get(conn_id).await {
                let mut client = client_arc.lock().await;
                match client.get_sftp().await {
                    Ok(mut sftp) => {
                        match sftp.read_dir(path).await {
                            Ok(dir) => {
                                let mut entries = Vec::new();
                                for entry in dir {
                                    entries.push(serde_json::json!({
                                        "name": entry.file_name(),
                                        "path": entry.path(),
                                        // "type": format!("{:?}", entry.file_type()),
                                    }));
                                }
                                let result = json!({ "content": [{ "type": "text", "text": serde_json::to_string(&entries).unwrap() }] });
                                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                                let _ = state.mcp_tx.send(resp.to_string());
                                return Json(resp);
                            }
                            Err(e) => {
                                let result = json!({ "isError": true, "content": [{ "type": "text", "text": format!("SFTP ReadDir Error: {}", e) }] });
                                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                                let _ = state.mcp_tx.send(resp.to_string());
                                return Json(resp);
                            }
                        }
                    }
                    Err(e) => {
                        let result = json!({ "isError": true, "content": [{ "type": "text", "text": format!("SFTP Init Error: {}", e) }] });
                        let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                        let _ = state.mcp_tx.send(resp.to_string());
                        return Json(resp);
                    }
                }
            } else {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Connection not found." }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }

        if name == "sftp_read_file" {
            let args = &params["arguments"];
            let conn_id = args["connection_id"].as_str().unwrap_or("");
            let path = args["path"].as_str().unwrap_or("");

            if let Some(client_arc) = state.connections.get(conn_id).await {
                let mut client = client_arc.lock().await;
                match client.get_sftp().await {
                    Ok(mut sftp) => {
                        match sftp.read(path).await {
                            Ok(data) => {
                                let content = String::from_utf8_lossy(&data).to_string();
                                let result = json!({ "content": [{ "type": "text", "text": content }] });
                                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                                let _ = state.mcp_tx.send(resp.to_string());
                                return Json(resp);
                            }
                            Err(e) => {
                                let result = json!({ "isError": true, "content": [{ "type": "text", "text": format!("SFTP Read Error: {}", e) }] });
                                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                                let _ = state.mcp_tx.send(resp.to_string());
                                return Json(resp);
                            }
                        }
                    }
                    Err(e) => {
                        let result = json!({ "isError": true, "content": [{ "type": "text", "text": format!("SFTP Init Error: {}", e) }] });
                        let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                        let _ = state.mcp_tx.send(resp.to_string());
                        return Json(resp);
                    }
                }
            } else {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Connection not found." }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }

        if name == "sftp_write_file" {
            let args = &params["arguments"];
            let conn_id = args["connection_id"].as_str().unwrap_or("");
            let path = args["path"].as_str().unwrap_or("");
            let content = args["content"].as_str().unwrap_or("");

            if let Some(client_arc) = state.connections.get(conn_id).await {
                let mut client = client_arc.lock().await;
                match client.get_sftp().await {
                    Ok(mut sftp) => {
                        match sftp.write(path, content.as_bytes()).await {
                            Ok(_) => {
                                let result = json!({ "content": [{ "type": "text", "text": "File written successfully." }] });
                                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                                let _ = state.mcp_tx.send(resp.to_string());
                                return Json(resp);
                            }
                            Err(e) => {
                                let result = json!({ "isError": true, "content": [{ "type": "text", "text": format!("SFTP Write Error: {}", e) }] });
                                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                                let _ = state.mcp_tx.send(resp.to_string());
                                return Json(resp);
                            }
                        }
                    }
                    Err(e) => {
                        let result = json!({ "isError": true, "content": [{ "type": "text", "text": format!("SFTP Init Error: {}", e) }] });
                        let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                        let _ = state.mcp_tx.send(resp.to_string());
                        return Json(resp);
                    }
                }
            } else {
                let result = json!({ "isError": true, "content": [{ "type": "text", "text": "Connection not found." }] });
                let resp = json!({ "jsonrpc": "2.0", "id": req.id, "result": result });
                let _ = state.mcp_tx.send(resp.to_string());
                return Json(resp);
            }
        }
"""

content = re.sub(r'(\s*Json\(json!\("ok"\)\)\n})', sftp_handlers + r'\1', content)

with open("src/mcp.rs", "w") as f:
    f.write(content)

