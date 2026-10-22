use crate::sync::file::write_private;
use crate::sync::{SyncHook, SyncSource, SyncState, values_of};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum KeychainItemType {
    Password,
    PrivateKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeychainItem {
    pub id: String,
    pub name: String,
    pub item_type: KeychainItemType,
    pub value: String,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

pub struct KeychainStore {
    items: Mutex<HashMap<String, KeychainItem>>,
    file_path: String,
    sync: SyncHook,
}

impl KeychainStore {
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

        KeychainStore {
            items: Mutex::new(items),
            file_path: file_path.to_string(),
            sync: SyncHook::default(),
        }
    }

    fn write_file(&self, items: &HashMap<String, KeychainItem>) {
        if let Ok(data) = serde_json::to_string_pretty(items) {
            let _ = write_private(Path::new(&self.file_path), data.as_bytes());
        }
    }

    fn save(&self, items: &HashMap<String, KeychainItem>) {
        self.write_file(items);
        self.sync.notify("keychain", items);
    }

    pub fn get_all(&self) -> Vec<KeychainItem> {
        let items = self.items.lock().unwrap();
        let mut list: Vec<KeychainItem> = items.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn get(&self, id: &str) -> Option<KeychainItem> {
        let items = self.items.lock().unwrap();
        items.get(id).cloned()
    }

    pub fn add(&self, mut item: KeychainItem) -> KeychainItem {
        if item.id.is_empty() {
            item.id = Uuid::new_v4().to_string();
        }
        let mut items = self.items.lock().unwrap();
        items.insert(item.id.clone(), item.clone());
        self.save(&items);
        item
    }

    pub fn update(&self, id: &str, item: KeychainItem) -> Option<KeychainItem> {
        let mut items = self.items.lock().unwrap();
        if items.contains_key(id) {
            let mut updated = item.clone();
            updated.id = id.to_string();
            items.insert(id.to_string(), updated.clone());
            self.save(&items);
            Some(updated)
        } else {
            None
        }
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

impl SyncSource for KeychainStore {
    fn collection(&self) -> &'static str {
        "keychain"
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

    fn put(&self, id: &str, mut record: Value, notify: bool) -> Result<Value, String> {
        // An item that arrives without its secret is a placeholder: the entry
        // exists so hosts can point at it, and its value is typed in here.
        if let Value::Object(m) = &mut record {
            m.entry("value").or_insert_with(|| Value::String(String::new()));
        }
        let mut item: KeychainItem = serde_json::from_value(record).map_err(|e| e.to_string())?;
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
