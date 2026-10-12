use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
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
}

pub struct KeychainStore {
    items: Mutex<HashMap<String, KeychainItem>>,
    file_path: String,
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
        }
    }

    fn save(&self, items: &HashMap<String, KeychainItem>) {
        if let Ok(data) = serde_json::to_string_pretty(items) {
            let _ = fs::write(&self.file_path, data);
        }
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
