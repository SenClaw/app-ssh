use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const RING_CAP: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: u64,
    pub ts: u64,
    pub level: String,
    pub source: String,
    pub action: String,
    pub host: Option<String>,
    pub message: String,
    pub meta: Option<serde_json::Value>,
}

pub struct LogStore {
    inner: Mutex<Inner>,
    tx: tokio::sync::broadcast::Sender<LogEntry>,
}

struct Inner {
    next_id: u64,
    ring: VecDeque<LogEntry>,
}

impl LogStore {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(256);
        Self {
            inner: Mutex::new(Inner {
                next_id: 1,
                ring: VecDeque::with_capacity(RING_CAP),
            }),
            tx,
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<LogEntry> {
        self.tx.subscribe()
    }

    pub fn push(
        &self,
        level: &str,
        source: &str,
        action: &str,
        host: Option<String>,
        message: impl Into<String>,
        meta: Option<serde_json::Value>,
    ) {
        let entry = {
            let mut g = self.inner.lock().unwrap();
            let id = g.next_id;
            g.next_id += 1;
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let entry = LogEntry {
                id,
                ts,
                level: level.to_string(),
                source: source.to_string(),
                action: action.to_string(),
                host,
                message: message.into(),
                meta,
            };
            if g.ring.len() >= RING_CAP {
                g.ring.pop_front();
            }
            g.ring.push_back(entry.clone());
            entry
        };
        let _ = self.tx.send(entry);
    }

    pub fn list(&self, limit: usize) -> Vec<LogEntry> {
        let g = self.inner.lock().unwrap();
        let n = g.ring.len();
        let start = if n > limit { n - limit } else { 0 };
        g.ring.iter().skip(start).cloned().collect()
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap();
        g.ring.clear();
    }

    /// Drop entries older than `max_age_ms`. Returns number removed.
    pub fn prune_older_than(&self, max_age_ms: u64) -> usize {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let cutoff = now.saturating_sub(max_age_ms);
        let mut g = self.inner.lock().unwrap();
        let before = g.ring.len();
        while let Some(front) = g.ring.front() {
            if front.ts < cutoff {
                g.ring.pop_front();
            } else {
                break;
            }
        }
        before - g.ring.len()
    }
}

pub fn info(
    store: &LogStore,
    source: &str,
    action: &str,
    host: Option<String>,
    msg: impl Into<String>,
) {
    store.push("info", source, action, host, msg, None);
}

pub fn warn(
    store: &LogStore,
    source: &str,
    action: &str,
    host: Option<String>,
    msg: impl Into<String>,
) {
    store.push("warn", source, action, host, msg, None);
}

pub fn error(
    store: &LogStore,
    source: &str,
    action: &str,
    host: Option<String>,
    msg: impl Into<String>,
) {
    store.push("error", source, action, host, msg, None);
}
