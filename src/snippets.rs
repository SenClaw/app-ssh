use crate::sync::file::write_private;
use crate::sync::{SyncHook, SyncSource, SyncState, values_of};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// A reusable shell command the user keeps around and fires into a terminal,
/// the way Termius' Snippets panel does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    /// The command line(s) sent to the shell. Multi-line scripts are allowed.
    pub script: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Epoch millis. Drives the newest-first ordering the panel shows.
    #[serde(default)]
    pub created_at: u64,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct SnippetStore {
    items: Mutex<HashMap<String, Snippet>>,
    file_path: String,
    sync: SyncHook,
}

impl SnippetStore {
    pub fn new(file_path: &str) -> Self {
        let items = if Path::new(file_path).exists() {
            if let Ok(data) = fs::read_to_string(file_path) {
                serde_json::from_str(&data).unwrap_or_else(|_| HashMap::new())
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        SnippetStore {
            items: Mutex::new(items),
            file_path: file_path.to_string(),
            sync: SyncHook::default(),
        }
    }

    fn write_file(&self, items: &HashMap<String, Snippet>) {
        if let Ok(data) = serde_json::to_string_pretty(items) {
            let _ = write_private(Path::new(&self.file_path), data.as_bytes());
        }
    }

    fn save(&self, items: &HashMap<String, Snippet>) {
        self.write_file(items);
        self.sync.notify("snippets", items);
    }

    /// Newest first — a snippet just saved should be at the top of the panel.
    pub fn get_all(&self) -> Vec<Snippet> {
        let items = self.items.lock().unwrap();
        let mut list: Vec<Snippet> = items.values().cloned().collect();
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.name.cmp(&b.name)));
        list
    }

    pub fn add(&self, mut item: Snippet) -> Snippet {
        if item.id.is_empty() {
            item.id = Uuid::new_v4().to_string();
        }
        if item.created_at == 0 {
            item.created_at = now_ms();
        }
        let mut items = self.items.lock().unwrap();
        items.insert(item.id.clone(), item.clone());
        self.save(&items);
        item
    }

    pub fn update(&self, id: &str, item: Snippet) -> Option<Snippet> {
        let mut items = self.items.lock().unwrap();
        let existing = items.get(id)?;
        let mut updated = item;
        updated.id = id.to_string();
        // Editing a snippet must not reshuffle the list under the user's cursor.
        updated.created_at = existing.created_at;
        items.insert(id.to_string(), updated.clone());
        self.save(&items);
        Some(updated)
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut items = self.items.lock().unwrap();
        if items.remove(id).is_some() {
            self.save(&items);
            true
        } else {
            false
        }
    }
}

impl SyncSource for SnippetStore {
    fn collection(&self) -> &'static str {
        "snippets"
    }

    fn attach(&self, state: Arc<SyncState>) {
        self.sync.attach(state);
    }

    fn live(&self) -> Vec<(String, Value)> {
        values_of(&self.items.lock().unwrap())
    }

    fn get(&self, id: &str) -> Option<Value> {
        let items = self.items.lock().unwrap();
        items.get(id).and_then(|i| serde_json::to_value(i).ok())
    }

    fn put(&self, id: &str, record: Value, notify: bool) -> Result<Value, String> {
        let mut item: Snippet = serde_json::from_value(record).map_err(|e| e.to_string())?;
        item.id = id.to_string();
        let mut items = self.items.lock().unwrap();
        items.insert(id.to_string(), item.clone());
        if notify {
            self.save(&items);
        } else {
            self.write_file(&items);
        }
        serde_json::to_value(&item).map_err(|e| e.to_string())
    }

    fn remove(&self, id: &str, notify: bool) -> bool {
        let mut items = self.items.lock().unwrap();
        let removed = items.remove(id).is_some();
        if removed {
            if notify {
                self.save(&items);
            } else {
                self.write_file(&items);
            }
        }
        removed
    }
}
