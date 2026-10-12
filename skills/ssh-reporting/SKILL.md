---
name: ssh-reporting
description: Báo cáo trạng thái và thống kê kết nối SSH
triggers:
  - "báo cáo ssh"
  - "ssh report"
  - "thống kê server"
---

# SSH System Reporting Skill

You are an expert Linux System Administrator AI. 
Use this skill when the user asks you to "check the system status", "report on the server", or "check disk/cpu/ram of an SSH server".

## Tool names & availability (read BEFORE claiming tools are missing)

Every tool in this skill lives on the Space-App MCP server **`ssh-manager-mcp`** (the "SSH Manager" app). The full tool identifier is always `mcp__ssh-manager-mcp__<tool>` — never a shortened form like `mcp__ssh__*` or `mcp__ssh-manager__*`.

- Load the schemas in ONE ToolSearch call:
  `select:mcp__ssh-manager-mcp__ssh_list_hosts,mcp__ssh-manager-mcp__ssh_start_connect_id,mcp__ssh-manager-mcp__ssh_start_connect,mcp__ssh-manager-mcp__ssh_execute_command,mcp__ssh-manager-mcp__ssh_close_connect`
  (ToolSearch is hyphen/underscore-insensitive, so `mcp__ssh_manager_mcp__...` also resolves.)
- If ToolSearch returns 0 matches with `deferred_total: 0`, the session's tool roster is empty — do NOT conclude the SSH MCP does not exist, and do NOT fall back to local shell/ssh. Report to the user that either (a) the SSH Manager Space App is not running / its MCP is not registered, or (b) this chat's `allowed_tools` whitelist is stripping MCP tools (a non-empty whitelist exposes only the listed tools), and stop.

## Instructions

1. **Find and Connect to the Target Host**:
   - Call the `mcp__ssh-manager-mcp__ssh_list_hosts` MCP tool to get the list of available hosts managed by SSH Manager.
   - Find the host that matches the user's request (by IP, name, or tags).
   - Use `mcp__ssh-manager-mcp__ssh_start_connect_id` with `host_id` = the exact `id` from `ssh_list_hosts` to start a stateful SSH session. This will return a `connection_id`. (Only if the server is NOT in the saved list, use `mcp__ssh-manager-mcp__ssh_start_connect` with explicit `host` + `user` instead.)

2. **Execute Diagnostic Commands**:
   Use the `mcp__ssh-manager-mcp__ssh_execute_command` MCP tool, passing the returned `connection_id`, to run the following commands on the target host:
   
   - **Disk Usage**: `df -sh /` or `df -h`
   - **Memory (RAM)**: `free -m`
   - **CPU & Load**: `uptime` and `top -bn1 | head -n 5`
   - **Network**: `ip a` or `ping -c 3 google.com`
   
   *Note: Remember to use `mcp__ssh-manager-mcp__ssh_close_connect` with the `connection_id` when finished to free up resources.*

3. **Generate the Report**:
   Compile the results into a professional Markdown report.
   Use clear headings, bullet points, and code blocks for raw command outputs where appropriate.
   
   Example format:
   ```markdown
   # System Health Report for [Host IP]

   ## 💾 Disk Space
   - Total/Used/Available summary...
   
   ## 🧠 Memory (RAM)
   - Total RAM and usage status...
   
   ## ⚡ CPU & Load Average
   - Current load average and CPU usage...
   
   ## 🌐 Network Status
   - IP addresses and basic connectivity...
   ```
