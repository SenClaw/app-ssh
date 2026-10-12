use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
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
}

pub struct HostStore {
    hosts: Mutex<HashMap<String, Host>>,
    file_path: String,
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
        }
    }

    fn save(&self, hosts: &HashMap<String, Host>) {
        if let Ok(data) = serde_json::to_string_pretty(hosts) {
            let _ = fs::write(&self.file_path, data);
        }
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
}

pub struct PortForwardingStore {
    rules: Mutex<HashMap<String, PortForwardingRule>>,
    file_path: String,
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
        }
    }

    fn save(&self, rules: &HashMap<String, PortForwardingRule>) {
        if let Ok(data) = serde_json::to_string_pretty(rules) {
            let _ = fs::write(&self.file_path, data);
        }
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
