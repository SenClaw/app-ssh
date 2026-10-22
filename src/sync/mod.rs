//! Keeping this app's data in step with another SenClaw's copy of it.
//!
//! The app does not talk to the other device. The SenClaw daemon does: it is
//! the one that pairs with a peer, encrypts the channel, asks the person, and
//! carries these calls across (`/api/sync/*`, reachable only with the app's
//! access token). What this module owns is what the data *means* — what makes
//! two records the same, who wins an edit, what must never leave this machine.
//!
//! Model: every record has an id that is the same on every device, and a stamp
//! (see `hlc`) saying when it last changed. Two devices exchange a digest of
//! `(id, stamp)` per collection and then the records that differ; the newer
//! stamp wins, on both sides, by the same rule — so the order calls arrive in,
//! and repeating them, change nothing. A deletion is a stamped tombstone.
//!
//! Things that do not travel: `local_fields` (whether a tunnel is open),
//! `secret_fields` unless the caller asks (passwords, keys), and the
//! `settings` collection (it holds this machine's command policy).

pub mod engine;
pub mod file;
pub mod hlc;
pub mod http;
pub mod spec;
pub mod state;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use serde::Serialize;
use serde_json::Value;

pub use state::SyncState;

/// One collection of records, as the sync engine sees it.
pub trait SyncSource: Send + Sync {
    fn collection(&self) -> &'static str;
    fn attach(&self, state: Arc<SyncState>);
    /// Every live record as JSON, local fields included.
    fn live(&self) -> Vec<(String, Value)>;
    fn get(&self, id: &str) -> Option<Value> {
        self.live().into_iter().find(|(i, _)| i == id).map(|(_, v)| v)
    }
    /// Insert or replace. `notify` false skips the change observer: the caller
    /// is applying a peer's record and registers its stamp itself. Returns the
    /// record as stored, which is what its fingerprint must be taken from.
    fn put(&self, id: &str, record: Value, notify: bool) -> Result<Value, String>;
    fn remove(&self, id: &str, notify: bool) -> bool;
}

/// What a store holds to tell sync about its edits.
#[derive(Default)]
pub struct SyncHook(OnceLock<Arc<SyncState>>);

impl SyncHook {
    pub fn attach(&self, state: Arc<SyncState>) {
        let _ = self.0.set(state);
    }

    /// Report the whole content of a collection after it was saved.
    pub fn notify<T: Serialize>(&self, collection: &str, items: &HashMap<String, T>) {
        if let Some(s) = self.0.get() {
            s.observe(collection, values_of(items), None);
        }
    }
}

pub fn values_of<T: Serialize>(items: &HashMap<String, T>) -> Vec<(String, Value)> {
    items
        .iter()
        .filter_map(|(id, v)| serde_json::to_value(v).ok().map(|v| (id.clone(), v)))
        .collect()
}

/// Bring sync up over the data already on disk. On the first run it backs the
/// files up, tightens their permissions, and stamps what is there with the
/// time each file last changed — so existing records are not mistaken for
/// edits made just now and do not beat a peer's genuinely newer ones.
pub fn start(dir: &Path, state: &Arc<SyncState>, sources: &[&dyn SyncSource]) {
    if state.is_fresh() {
        file::backup_once(dir);
    }
    file::harden(dir);
    for s in sources {
        s.attach(state.clone());
        let adopt = if state.is_fresh() {
            file::mtime_ms(&dir.join(format!("{}.json", s.collection())))
        } else {
            None
        };
        state.observe(s.collection(), s.live(), adopt);
    }
}

#[cfg(test)]
pub mod testutil {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sshm-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A clock the test moves by hand.
    pub fn manual_clock(start: u64) -> (Box<dyn Fn() -> u64 + Send + Sync>, Arc<AtomicU64>) {
        let cell = Arc::new(AtomicU64::new(start));
        let reader = cell.clone();
        (Box::new(move || reader.load(Ordering::SeqCst)), cell)
    }
}

#[cfg(test)]
mod tests;
