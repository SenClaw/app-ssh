//! The sync bookkeeping, kept beside the data rather than inside it.
//!
//! `sync.json` records, per collection, the stamp and content fingerprint of
//! every live record and a tombstone for every deleted one. The collections'
//! own files keep their old shape on purpose: an older build of the app reads
//! them with `unwrap_or_else(|_| HashMap::new())`, so a tombstone written into
//! `hosts.json` would make that build see an empty file and overwrite it.
//!
//! Edits are noticed by comparing fingerprints, not by every writer calling in:
//! the stores report their whole content after each save, and anything that
//! changed — including an edit made while sync was not running, or by hand —
//! gets stamped. A record whose fingerprint still matches keeps its stamp, which
//! is what lets a record applied from a peer keep the peer's stamp.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::file::write_private;
use super::hlc::{Clock, Hlc};
use super::spec::{content_hash, spec_of, strip_local};

/// How long a deletion is remembered. A peer away longer than this could bring
/// the record back, so sync of a peer last seen before this must be reviewed.
pub const TOMBSTONE_TTL_MS: u64 = 90 * 24 * 3600 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    pub hlc: String,
    pub hash: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CollState {
    #[serde(default)]
    meta: BTreeMap<String, Meta>,
    /// id → stamp of the deletion.
    #[serde(default)]
    tombstones: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Persisted {
    version: u32,
    device_id: String,
    #[serde(default)]
    clock_ms: u64,
    #[serde(default)]
    clock_counter: u32,
    #[serde(default)]
    collections: BTreeMap<String, CollState>,
}

struct Inner {
    data: Persisted,
    clock: Clock,
    path: PathBuf,
}

/// One entry of a collection's digest: all a peer needs to decide what to ask for.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DigestEntry {
    pub id: String,
    pub hlc: String,
    pub deleted: bool,
}

pub struct SyncState {
    inner: Mutex<Inner>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
    /// `sync.json` did not exist when this was opened.
    fresh: bool,
}

pub fn system_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl SyncState {
    pub fn open(dir: &Path) -> Arc<SyncState> {
        Self::open_with_clock(dir, Box::new(system_now_ms))
    }

    pub fn open_with_clock(dir: &Path, now: Box<dyn Fn() -> u64 + Send + Sync>) -> Arc<SyncState> {
        let path = dir.join("sync.json");
        let text = std::fs::read_to_string(&path).ok();
        let loaded: Option<Persisted> = text.as_deref().and_then(|t| serde_json::from_str(t).ok());
        if loaded.is_none() && text.is_some() {
            // Unreadable bookkeeping is started over, but kept for whoever has to find out why.
            let _ = std::fs::rename(&path, dir.join(format!("sync.json.corrupt-{}", now())));
        }
        let fresh = loaded.is_none();
        let mut data = loaded.unwrap_or_else(|| Persisted {
            version: 1,
            device_id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            clock_ms: 0,
            clock_counter: 0,
            collections: BTreeMap::new(),
        });
        let cutoff = now().saturating_sub(TOMBSTONE_TTL_MS);
        for coll in data.collections.values_mut() {
            coll.tombstones.retain(|_, hlc| Hlc::parse(hlc).map(|h| h.ms >= cutoff).unwrap_or(false));
        }
        let clock = Clock { ms: data.clock_ms, counter: data.clock_counter };
        let state = SyncState { inner: Mutex::new(Inner { data, clock, path }), now, fresh };
        state.save(&state.inner.lock().unwrap());
        Arc::new(state)
    }

    pub fn is_fresh(&self) -> bool {
        self.fresh
    }

    pub fn now_ms(&self) -> u64 {
        (self.now)()
    }

    pub fn device_id(&self) -> String {
        self.inner.lock().unwrap().data.device_id.clone()
    }

    fn save(&self, g: &Inner) {
        // sync.json is not secret, but it sits beside files that are.
        if let Ok(text) = serde_json::to_string_pretty(&g.data) {
            let _ = write_private(&g.path, text.as_bytes());
        }
    }

    fn tick_locked(&self, g: &mut Inner) -> Hlc {
        let device = g.data.device_id.clone();
        let h = g.clock.tick((self.now)(), &device);
        g.data.clock_ms = g.clock.ms;
        g.data.clock_counter = g.clock.counter;
        h
    }

    /// A fresh stamp for a local event.
    pub fn tick(&self) -> Hlc {
        let mut g = self.inner.lock().unwrap();
        let h = self.tick_locked(&mut g);
        self.save(&g);
        h
    }

    /// Note a stamp that arrived from a peer so later local edits sort after it.
    pub fn receive(&self, remote: &Hlc) {
        let mut g = self.inner.lock().unwrap();
        g.clock.receive(remote, (self.now)());
        g.data.clock_ms = g.clock.ms;
        g.data.clock_counter = g.clock.counter;
        // Saved with the record this stamp came with, which follows at once.
    }

    /// Compare a collection's current content with what was last recorded, and
    /// stamp what differs: new or edited records get a fresh stamp, vanished
    /// ones a tombstone. `adopt_ms` (the first run over existing data) gives
    /// new records an old stamp instead, so data that was already there does
    /// not look edited just now.
    pub fn observe(&self, collection: &str, records: Vec<(String, Value)>, adopt_ms: Option<u64>) {
        let Some(spec) = spec_of(collection) else { return };
        let mut g = self.inner.lock().unwrap();
        let mut changed = false;
        let mut seen = BTreeSet::new();
        for (id, record) in records {
            seen.insert(id.clone());
            let hash = content_hash(&strip_local(spec, &record));
            let (known, same) = match g.data.collections.get(collection).and_then(|c| c.meta.get(&id)) {
                Some(m) => (true, m.hash == hash),
                None => (false, false),
            };
            if same {
                continue;
            }
            let hlc = match adopt_ms {
                Some(ms) if !known => Hlc { ms, counter: 0, device: g.data.device_id.clone() },
                _ => self.tick_locked(&mut g),
            };
            let coll = g.data.collections.entry(collection.to_string()).or_default();
            coll.tombstones.remove(&id);
            coll.meta.insert(id, Meta { hlc: hlc.to_string(), hash });
            changed = true;
        }
        let gone: Vec<String> = g
            .data
            .collections
            .get(collection)
            .map(|c| c.meta.keys().filter(|id| !seen.contains(*id)).cloned().collect())
            .unwrap_or_default();
        for id in gone {
            let hlc = self.tick_locked(&mut g);
            let coll = g.data.collections.entry(collection.to_string()).or_default();
            coll.meta.remove(&id);
            coll.tombstones.insert(id, hlc.to_string());
            changed = true;
        }
        if changed {
            self.save(&g);
        }
    }

    /// The stamp this device holds for an id: its live record's, or its tombstone's.
    pub fn version_of(&self, collection: &str, id: &str) -> Option<Hlc> {
        let g = self.inner.lock().unwrap();
        let coll = g.data.collections.get(collection)?;
        let text = coll.meta.get(id).map(|m| &m.hlc).or_else(|| coll.tombstones.get(id))?;
        Hlc::parse(text)
    }

    /// Record that `record` was stored with the stamp it arrived with.
    pub fn register_live(&self, collection: &str, id: &str, hlc: &Hlc, record: &Value) {
        let Some(spec) = spec_of(collection) else { return };
        let mut g = self.inner.lock().unwrap();
        let hash = content_hash(&strip_local(spec, record));
        let coll = g.data.collections.entry(collection.to_string()).or_default();
        coll.tombstones.remove(id);
        coll.meta.insert(id.to_string(), Meta { hlc: hlc.to_string(), hash });
        self.save(&g);
    }

    /// Record a deletion with the stamp it arrived with.
    pub fn register_deleted(&self, collection: &str, id: &str, hlc: &Hlc) {
        let mut g = self.inner.lock().unwrap();
        let coll = g.data.collections.entry(collection.to_string()).or_default();
        coll.meta.remove(id);
        coll.tombstones.insert(id.to_string(), hlc.to_string());
        self.save(&g);
    }

    /// Every id this device knows of in a collection, live or deleted.
    pub fn digest(&self, collection: &str) -> Vec<DigestEntry> {
        let g = self.inner.lock().unwrap();
        let Some(coll) = g.data.collections.get(collection) else { return Vec::new() };
        let live = coll.meta.iter().map(|(id, m)| DigestEntry { id: id.clone(), hlc: m.hlc.clone(), deleted: false });
        let dead = coll.tombstones.iter().map(|(id, h)| DigestEntry { id: id.clone(), hlc: h.clone(), deleted: true });
        let mut all: Vec<DigestEntry> = live.chain(dead).collect();
        all.sort_by(|a, b| a.id.cmp(&b.id));
        all
    }

    pub fn clock_ms(&self) -> u64 {
        self.inner.lock().unwrap().clock.ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::testutil::{manual_clock, scratch};
    use serde_json::json;
    use std::sync::atomic::Ordering;

    fn host(id: &str, name: &str) -> (String, Value) {
        (id.to_string(), json!({"id": id, "name": name, "host": "h", "port": 22, "user": "u"}))
    }

    #[test]
    fn unchanged_records_keep_their_stamp_and_edits_get_a_new_one() {
        let dir = scratch("state");
        let (now, clock) = manual_clock(1_000);
        let s = SyncState::open_with_clock(&dir, now);
        s.observe("hosts", vec![host("a", "one")], None);
        let first = s.version_of("hosts", "a").unwrap();
        clock.store(2_000, Ordering::SeqCst);
        s.observe("hosts", vec![host("a", "one")], None);
        assert_eq!(s.version_of("hosts", "a").unwrap(), first);
        s.observe("hosts", vec![host("a", "two")], None);
        assert!(s.version_of("hosts", "a").unwrap() > first);
    }

    #[test]
    fn a_vanished_record_becomes_a_tombstone_and_a_return_clears_it() {
        let dir = scratch("tomb");
        let (now, clock) = manual_clock(1_000);
        let s = SyncState::open_with_clock(&dir, now);
        s.observe("hosts", vec![host("a", "one")], None);
        clock.store(2_000, Ordering::SeqCst);
        s.observe("hosts", vec![], None);
        let d = s.digest("hosts");
        assert_eq!(d.len(), 1);
        assert!(d[0].deleted);
        clock.store(3_000, Ordering::SeqCst);
        s.observe("hosts", vec![host("a", "one")], None);
        assert!(!s.digest("hosts")[0].deleted);
    }

    #[test]
    fn first_run_stamps_existing_data_with_the_given_time_not_now() {
        let dir = scratch("adopt");
        let (now, _) = manual_clock(50_000);
        let s = SyncState::open_with_clock(&dir, now);
        s.observe("hosts", vec![host("a", "one")], Some(7_000));
        assert_eq!(s.version_of("hosts", "a").unwrap().ms, 7_000);
        // An edit after that is stamped with the real time.
        s.observe("hosts", vec![host("a", "two")], Some(7_000));
        assert_eq!(s.version_of("hosts", "a").unwrap().ms, 50_000);
    }

    #[test]
    fn local_fields_do_not_count_as_an_edit() {
        let dir = scratch("local");
        let (now, clock) = manual_clock(1_000);
        let s = SyncState::open_with_clock(&dir, now);
        let rule = |active: bool| {
            vec![("r".to_string(), json!({"id": "r", "host_id": "h", "local_port": 1, "active": active}))]
        };
        s.observe("port_forwarding", rule(false), None);
        let before = s.version_of("port_forwarding", "r").unwrap();
        clock.store(9_000, Ordering::SeqCst);
        s.observe("port_forwarding", rule(true), None);
        assert_eq!(s.version_of("port_forwarding", "r").unwrap(), before);
    }

    #[test]
    fn state_and_device_survive_a_restart_and_a_backwards_wall_clock() {
        let dir = scratch("reopen");
        let (now, _) = manual_clock(10_000);
        let s = SyncState::open_with_clock(&dir, now);
        s.observe("hosts", vec![host("a", "one")], None);
        let (stamp, device) = (s.version_of("hosts", "a").unwrap(), s.device_id());
        let (earlier, _) = manual_clock(1_000);
        let again = SyncState::open_with_clock(&dir, earlier);
        assert!(!again.is_fresh());
        assert_eq!(again.device_id(), device);
        assert_eq!(again.version_of("hosts", "a").unwrap(), stamp);
        assert!(again.tick() > stamp);
    }

    #[test]
    fn an_unreadable_file_is_set_aside_and_sync_starts_over() {
        let dir = scratch("corrupt");
        std::fs::write(dir.join("sync.json"), "{ not json").unwrap();
        let (now, _) = manual_clock(1_234);
        let s = SyncState::open_with_clock(&dir, now);
        assert!(s.is_fresh());
        assert_eq!(std::fs::read_to_string(dir.join("sync.json.corrupt-1234")).unwrap(), "{ not json");
        assert!(serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(dir.join("sync.json")).unwrap()).is_ok());
    }

    #[test]
    fn old_tombstones_are_dropped_on_open() {
        let dir = scratch("ttl");
        let (now, _) = manual_clock(1_000);
        let s = SyncState::open_with_clock(&dir, now);
        s.observe("hosts", vec![host("a", "one")], None);
        s.observe("hosts", vec![], None);
        let (later, _) = manual_clock(1_000 + TOMBSTONE_TTL_MS + 5_000);
        let again = SyncState::open_with_clock(&dir, later);
        assert!(again.digest("hosts").is_empty());
    }
}
