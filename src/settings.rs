use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub log_retention_seconds: u64,
    /// Gate for the `ssh_execute_command` MCP tool.
    /// "off"       — no filtering
    /// "allowlist" — only commands in `ssh_allowed_commands` are permitted
    /// "denylist"  — everything is permitted except commands in `ssh_denied_commands`
    #[serde(default = "default_ssh_policy")]
    pub ssh_command_policy: String,
    #[serde(default)]
    pub ssh_allowed_commands: Vec<String>,
    #[serde(default)]
    pub ssh_denied_commands: Vec<String>,
}

fn default_theme() -> String {
    "dark".to_string()
}
fn default_ssh_policy() -> String {
    "off".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            log_retention_seconds: 0, // 0 = never auto-clear
            ssh_command_policy: default_ssh_policy(),
            ssh_allowed_commands: Vec::new(),
            ssh_denied_commands: vec![
                "rm".into(),
                "mkfs".into(),
                "dd".into(),
                "shutdown".into(),
                "reboot".into(),
                "halt".into(),
                "poweroff".into(),
                ":(){".into(),
            ],
        }
    }
}

/// Verdict returned by `Settings::check_ssh_command`.
pub enum CmdVerdict {
    Allow,
    Deny(&'static str),
}

pub struct SettingsStore {
    file_path: PathBuf,
    inner: Mutex<Settings>,
}

impl SettingsStore {
    pub fn new(file_path: PathBuf) -> Self {
        let inner = fs::read_to_string(&file_path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default();
        Self {
            file_path,
            inner: Mutex::new(inner),
        }
    }

    pub fn get(&self) -> Settings {
        self.inner.lock().unwrap().clone()
    }

    pub fn set(&self, new: Settings) -> Settings {
        {
            let mut g = self.inner.lock().unwrap();
            *g = new.clone();
        }
        let _ = fs::write(
            &self.file_path,
            serde_json::to_string_pretty(&new).unwrap_or_default(),
        );
        new
    }

    pub fn log_retention(&self) -> u64 {
        self.inner.lock().unwrap().log_retention_seconds
    }

    /// Check a command line (e.g. "ls -la /tmp") against the configured SSH policy.
    /// Matching is done on the first whitespace-separated token (the program name).
    pub fn check_ssh_command(&self, command_line: &str) -> CmdVerdict {
        let g = self.inner.lock().unwrap();
        let head = command_line.trim().split_whitespace().next().unwrap_or("");
        if head.is_empty() {
            return CmdVerdict::Deny("empty command");
        }
        match g.ssh_command_policy.as_str() {
            "allowlist" => {
                if g.ssh_allowed_commands.iter().any(|c| c == head) {
                    CmdVerdict::Allow
                } else {
                    CmdVerdict::Deny("command not in allowlist")
                }
            }
            "denylist" => {
                if g.ssh_denied_commands.iter().any(|c| c == head) {
                    CmdVerdict::Deny("command is in denylist")
                } else {
                    CmdVerdict::Allow
                }
            }
            _ => CmdVerdict::Allow,
        }
    }
}
