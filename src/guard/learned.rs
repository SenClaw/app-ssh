//! The allow list the guard keeps itself: exact commands that were cleared
//! once, so the same command runs at once the next time.
//!
//! An entry is a whole command line (whitespace collapsed), never a program
//! name — `docker ps` being harmless says nothing about `docker rm`. Entries
//! come from the AI review (both reviewers agreed it only looks) or from the
//! admin ("Approve & remember"). The file sits next to `settings.json` and,
//! like it, never syncs to another device: what this machine trusts is its own.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::shell::normalize;
use crate::sync::file::write_private;

/// Oldest AI-learned entries go first past this; admin entries are kept.
const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearnedCommand {
    pub command: String,
    /// `"ai"` or `"admin"`.
    pub source: String,
    #[serde(default)]
    pub reason: String,
    /// Epoch milliseconds.
    pub added_at: u64,
}

pub struct LearnedStore {
    path: PathBuf,
    items: Mutex<Vec<LearnedCommand>>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl LearnedStore {
    pub fn open(path: &Path) -> Self {
        let items = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<LearnedCommand>>(&s).ok())
            .unwrap_or_default();
        Self {
            path: path.to_path_buf(),
            items: Mutex::new(items),
        }
    }

    pub fn list(&self) -> Vec<LearnedCommand> {
        let mut v = self.items.lock().unwrap().clone();
        v.sort_by(|a, b| b.added_at.cmp(&a.added_at));
        v
    }

    pub fn get(&self, command: &str) -> Option<LearnedCommand> {
        let key = normalize(command);
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.command == key)
            .cloned()
    }

    /// Add or refresh an entry. An admin entry is never downgraded to an AI one.
    pub fn add(&self, command: &str, source: &str, reason: &str) -> LearnedCommand {
        let key = normalize(command);
        let entry = {
            let mut items = self.items.lock().unwrap();
            let entry = match items.iter_mut().find(|c| c.command == key) {
                Some(existing) => {
                    if source == "admin" {
                        existing.source = "admin".into();
                        existing.reason = reason.to_string();
                    }
                    existing.clone()
                }
                None => {
                    let e = LearnedCommand {
                        command: key,
                        source: source.to_string(),
                        reason: reason.to_string(),
                        added_at: now_ms(),
                    };
                    items.push(e.clone());
                    e
                }
            };
            while items.len() > MAX_ENTRIES {
                let oldest_ai = items
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.source == "ai")
                    .min_by_key(|(_, c)| c.added_at)
                    .map(|(i, _)| i);
                match oldest_ai {
                    Some(i) => {
                        items.remove(i);
                    }
                    None => break,
                }
            }
            entry
        };
        self.save();
        entry
    }

    pub fn remove(&self, command: &str) -> bool {
        let key = normalize(command);
        let removed = {
            let mut items = self.items.lock().unwrap();
            let before = items.len();
            items.retain(|c| c.command != key);
            items.len() != before
        };
        if removed {
            self.save();
        }
        removed
    }

    fn save(&self) {
        let data = serde_json::to_vec_pretty(&*self.items.lock().unwrap()).unwrap_or_default();
        if let Err(e) = write_private(&self.path, &data) {
            eprintln!("could not save {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (LearnedStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("ssh-learned-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("learned_commands.json");
        (LearnedStore::open(&path), path)
    }

    #[test]
    fn entries_match_whole_commands_and_survive_a_restart() {
        let (s, path) = store();
        s.add("docker   ps -a", "ai", "lists containers");
        assert!(s.get("docker ps -a").is_some());
        assert!(s.get("docker ps").is_none());
        assert!(s.get("docker rm -f web").is_none());
        let again = LearnedStore::open(&path);
        assert_eq!(again.list().len(), 1);
        assert!(again.remove("docker ps -a"));
        assert!(LearnedStore::open(&path).list().is_empty());
    }

    #[test]
    fn an_admin_entry_is_not_downgraded() {
        let (s, _) = store();
        s.add("systemctl restart nginx", "admin", "approved");
        s.add("systemctl restart nginx", "ai", "");
        assert_eq!(s.get("systemctl restart nginx").unwrap().source, "admin");
    }
}
