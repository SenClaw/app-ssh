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
    /// Gate for the `ssh_execute_command` MCP tool (see `crate::guard`).
    /// "off"       — no filtering
    /// "allowlist" — only programs in `ssh_allowed_commands` are permitted
    /// "denylist"  — everything is permitted except programs in `ssh_denied_commands`
    /// "smart"     — read-only commands run, risky ones wait for the admin,
    ///               the rest are reviewed by the AI and System One
    #[serde(default = "default_ssh_policy")]
    pub ssh_command_policy: String,
    #[serde(default)]
    pub ssh_allowed_commands: Vec<String>,
    /// The denylist; in "smart" mode, programs that always wait for the admin.
    #[serde(default)]
    pub ssh_denied_commands: Vec<String>,
    /// "smart": add a command both reviewers cleared to the learned list.
    #[serde(default = "default_true")]
    pub ssh_ai_learn: bool,
    /// "smart": ask System One as well as the model.
    #[serde(default = "default_true")]
    pub ssh_system_one: bool,
    /// "smart": System One's "changes the server" counts from this confidence.
    #[serde(default = "default_system_one_floor")]
    pub ssh_system_one_floor: f64,
}

fn default_theme() -> String {
    "dark".to_string()
}
fn default_ssh_policy() -> String {
    "smart".to_string()
}
fn default_true() -> bool {
    true
}
fn default_system_one_floor() -> f64 {
    0.6
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
            ssh_ai_learn: true,
            ssh_system_one: true,
            ssh_system_one_floor: default_system_one_floor(),
        }
    }
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
}
