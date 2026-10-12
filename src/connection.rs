use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::client::SshClient;

pub struct ConnectionManager {
    connections: Mutex<HashMap<String, Arc<Mutex<SshClient>>>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    pub async fn add(&self, client: SshClient) -> String {
        let id = Uuid::new_v4().to_string();
        self.connections
            .lock()
            .await
            .insert(id.clone(), Arc::new(Mutex::new(client)));
        id
    }

    pub async fn get(&self, id: &str) -> Option<Arc<Mutex<SshClient>>> {
        self.connections.lock().await.get(id).cloned()
    }

    pub async fn remove(&self, id: &str) -> bool {
        self.connections.lock().await.remove(id).is_some()
    }

    pub async fn list(&self) -> Vec<String> {
        self.connections.lock().await.keys().cloned().collect()
    }
}
