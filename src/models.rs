use crate::sync::file::write_private;
use crate::sync::{SyncHook, SyncSource, SyncState, values_of};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub keychain_id: Option<String>,
    pub tags: Vec<String>,
    /// Fields a newer build wrote that this one does not know. Kept so that a
    /// record synced through an older build comes out the other side whole.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

pub struct HostStore {
    hosts: Mutex<HashMap<String, Host>>,
    file_path: String,
    sync: SyncHook,
}

impl HostStore {
    pub fn new(file_path: &str) -> Self {
        let hosts = if Path::new(file_path).exists() {
            if let Ok(data) = fs::read_to_string(file_path) {
                serde_json::from_str(&data).unwrap_or_else(|_| HashMap::new())
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        HostStore {
            hosts: Mutex::new(hosts),
            file_path: file_path.to_string(),
            sync: SyncHook::default(),
        }
    }

    fn write_file(&self, hosts: &HashMap<String, Host>) {
        if let Ok(data) = serde_json::to_string_pretty(hosts) {
            let _ = write_private(Path::new(&self.file_path), data.as_bytes());
        }
    }

    fn save(&self, hosts: &HashMap<String, Host>) {
        self.write_file(hosts);
        self.sync.notify("hosts", hosts);
    }

    pub fn get_all(&self) -> Vec<Host> {
        let hosts = self.hosts.lock().unwrap();
        let mut list: Vec<Host> = hosts.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn get(&self, id: &str) -> Option<Host> {
        let hosts = self.hosts.lock().unwrap();
        hosts.get(id).cloned()
    }

    pub fn add(&self, mut host: Host) -> Host {
        if host.id.is_empty() {
            host.id = Uuid::new_v4().to_string();
        }
        let mut hosts = self.hosts.lock().unwrap();
        hosts.insert(host.id.clone(), host.clone());
        self.save(&hosts);
        host
    }

    pub fn update(&self, id: &str, host: Host) -> Option<Host> {
        let mut hosts = self.hosts.lock().unwrap();
        if hosts.contains_key(id) {
            let mut updated = host.clone();
            updated.id = id.to_string();
            hosts.insert(id.to_string(), updated.clone());
            self.save(&hosts);
            Some(updated)
        } else {
            None
        }
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut hosts = self.hosts.lock().unwrap();
        if hosts.remove(id).is_some() {
            self.save(&hosts);
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortForwardingRule {
    pub id: String,
    pub name: String,
    pub local_port: u16,
    pub bind_address: String,
    pub host_id: String,
    pub destination_address: String,
    pub destination_port: u16,
    #[serde(default)]
    pub active: bool,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

pub struct PortForwardingStore {
    rules: Mutex<HashMap<String, PortForwardingRule>>,
    file_path: String,
    sync: SyncHook,
}

impl PortForwardingStore {
    pub fn new(file_path: &str) -> Self {
        let rules = if Path::new(file_path).exists() {
            if let Ok(data) = fs::read_to_string(file_path) {
                serde_json::from_str(&data).unwrap_or_else(|_| HashMap::new())
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        PortForwardingStore {
            rules: Mutex::new(rules),
            file_path: file_path.to_string(),
            sync: SyncHook::default(),
        }
    }

    fn write_file(&self, rules: &HashMap<String, PortForwardingRule>) {
        if let Ok(data) = serde_json::to_string_pretty(rules) {
            let _ = write_private(Path::new(&self.file_path), data.as_bytes());
        }
    }

    fn save(&self, rules: &HashMap<String, PortForwardingRule>) {
        self.write_file(rules);
        self.sync.notify("port_forwarding", rules);
    }

    pub fn get_all(&self) -> Vec<PortForwardingRule> {
        let rules = self.rules.lock().unwrap();
        let mut list: Vec<PortForwardingRule> = rules.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn get(&self, id: &str) -> Option<PortForwardingRule> {
        let rules = self.rules.lock().unwrap();
        rules.get(id).cloned()
    }

    pub fn add(&self, mut rule: PortForwardingRule) -> PortForwardingRule {
        if rule.id.is_empty() {
            rule.id = Uuid::new_v4().to_string();
        }
        let mut rules = self.rules.lock().unwrap();
        rules.insert(rule.id.clone(), rule.clone());
        self.save(&rules);
        rule
    }

    pub fn update(&self, id: &str, rule: PortForwardingRule) -> Option<PortForwardingRule> {
        let mut rules = self.rules.lock().unwrap();
        if rules.contains_key(id) {
            let mut updated = rule.clone();
            updated.id = id.to_string();
            rules.insert(id.to_string(), updated.clone());
            self.save(&rules);
            Some(updated)
        } else {
            None
        }
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut rules = self.rules.lock().unwrap();
        if rules.remove(id).is_some() {
            self.save(&rules);
            true
        } else {
            false
        }
    }

    pub fn set_active(&self, id: &str, active: bool) -> bool {
        let mut rules = self.rules.lock().unwrap();
        if let Some(rule) = rules.get_mut(id) {
            rule.active = active;
            self.save(&rules);
            true
        } else {
            false
        }
    }
}

impl SyncSource for HostStore {
    fn collection(&self) -> &'static str {
        "hosts"
    }

    fn attach(&self, state: Arc<SyncState>) {
        self.sync.attach(state);
    }

    fn live(&self) -> Vec<(String, Value)> {
        values_of(&self.hosts.lock().unwrap())
    }

    fn get(&self, id: &str) -> Option<Value> {
        let hosts = self.hosts.lock().unwrap();
        hosts.get(id).and_then(|h| serde_json::to_value(h).ok())
    }

    fn put(&self, id: &str, record: Value, notify: bool) -> Result<Value, String> {
        let mut host: Host = serde_json::from_value(record).map_err(|e| e.to_string())?;
        host.id = id.to_string();
        let mut hosts = self.hosts.lock().unwrap();
        hosts.insert(id.to_string(), host.clone());
        if notify {
            self.save(&hosts);
        } else {
            self.write_file(&hosts);
        }
        serde_json::to_value(&host).map_err(|e| e.to_string())
    }

    fn remove(&self, id: &str, notify: bool) -> bool {
        let mut hosts = self.hosts.lock().unwrap();
        let removed = hosts.remove(id).is_some();
        if removed {
            if notify {
                self.save(&hosts);
            } else {
                self.write_file(&hosts);
            }
        }
        removed
    }
}

impl SyncSource for PortForwardingStore {
    fn collection(&self) -> &'static str {
        "port_forwarding"
    }

    fn attach(&self, state: Arc<SyncState>) {
        self.sync.attach(state);
    }

    fn live(&self) -> Vec<(String, Value)> {
        values_of(&self.rules.lock().unwrap())
    }

    fn get(&self, id: &str) -> Option<Value> {
        let rules = self.rules.lock().unwrap();
        rules.get(id).and_then(|r| serde_json::to_value(r).ok())
    }

    fn put(&self, id: &str, record: Value, notify: bool) -> Result<Value, String> {
        let mut rule: PortForwardingRule = serde_json::from_value(record).map_err(|e| e.to_string())?;
        rule.id = id.to_string();
        let mut rules = self.rules.lock().unwrap();
        // Whether a tunnel is open is this machine's own business: a rule that
        // arrives is off, and one that is already here keeps its state.
        rule.active = rules.get(id).map(|r| r.active).unwrap_or(false);
        rules.insert(id.to_string(), rule.clone());
        if notify {
            self.save(&rules);
        } else {
            self.write_file(&rules);
        }
        serde_json::to_value(&rule).map_err(|e| e.to_string())
    }

    fn remove(&self, id: &str, notify: bool) -> bool {
        let mut rules = self.rules.lock().unwrap();
        let removed = rules.remove(id).is_some();
        if removed {
            if notify {
                self.save(&rules);
            } else {
                self.write_file(&rules);
            }
        }
        removed
    }
}
