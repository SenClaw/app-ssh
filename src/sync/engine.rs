//! The four operations a peer's sync needs from this app: describe the data,
//! say what is here, hand records out, and take records in.
//!
//! `apply` decides every record by one rule that does not depend on who sent
//! it or in what order: the newer stamp wins. That is what makes a repeated or
//! reordered batch harmless, and what lets two devices that both pull and push
//! end up identical without anyone arbitrating.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::SyncSource;
use super::hlc::Hlc;
use super::spec::{
    CollectionSpec, SPECS, content_hash, match_key_of, spec_index, spec_of, strip_local, strip_secrets, valid_id,
};
use super::state::SyncState;

/// A record stamped further ahead than this is refused: one device with a wrong
/// clock would otherwise win every conflict from then on.
pub const MAX_DRIFT_MS: u64 = 10 * 60 * 1000;
pub const MAX_RECORDS: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WireRecord {
    pub collection: String,
    pub id: String,
    pub hlc: String,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub data: Value,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyRequest {
    pub records: Vec<WireRecord>,
    /// Work out what would happen and change nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// Accept secret fields the sender included. Off: they are dropped and the
    /// secrets this device already has are kept.
    #[serde(default)]
    pub secrets: bool,
    /// Treat a record with the same match key as one already here as the same
    /// thing, and settle on one id. Off: report it as a duplicate and add nothing.
    #[serde(default)]
    pub adopt_matches: bool,
    /// Apply changes that redirect where stored credentials are used.
    #[serde(default)]
    pub allow_risky: bool,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ApplyItem {
    pub collection: String,
    pub id: String,
    /// added, updated, unchanged, deleted, tombstoned, adopted (merged into an
    /// existing record), skipped, held (needs a person), duplicate, rejected, error.
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl ApplyItem {
    fn new(collection: &str, id: &str, status: &'static str) -> Self {
        ApplyItem { collection: collection.into(), id: id.into(), status, reason: None, matched: None, warnings: Vec::new() }
    }
    fn reason(mut self, r: impl Into<String>) -> Self {
        self.reason = Some(r.into());
        self
    }
    fn matched(mut self, id: &str) -> Self {
        self.matched = Some(id.into());
        self
    }
    fn warnings(mut self, w: Vec<String>) -> Self {
        self.warnings = w;
        self
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub dry_run: bool,
    pub results: Vec<ApplyItem>,
    pub summary: BTreeMap<&'static str, usize>,
}

#[derive(Debug, PartialEq)]
pub enum SyncError {
    UnknownCollection(String),
    TooMany,
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::UnknownCollection(c) => write!(f, "`{c}` is not a syncable collection"),
            SyncError::TooMany => write!(f, "more than {MAX_RECORDS} records in one call"),
        }
    }
}

/// What earlier records of the same batch have already settled.
#[derive(Default)]
struct Batch {
    /// (collection, id the sender used) → the id kept here.
    id_map: HashMap<(String, String), String>,
    /// Dry run only: ids that would exist by now.
    planned: HashSet<(String, String)>,
    /// Records this batch turned away for a person to decide. Whatever points
    /// at one waits with it rather than landing with a reference to nothing.
    held: HashSet<(String, String)>,
}

/// Whether applying `after` over `before` would send stored credentials
/// somewhere new: the record carries credentials (its own, or a keychain item)
/// before or after, and one of the fields that say where they go changed. A
/// record that is new counts as changed from nothing, so a host that arrives
/// already pointing at a keychain item waits for a person like any other.
fn redirects(spec: &CollectionSpec, before: &Map<String, Value>, after: &Map<String, Value>) -> bool {
    let null = Value::Null;
    let carries = (spec.has_credentials)(before) || (spec.has_credentials)(after);
    let moved = spec.guard_fields.iter().any(|f| before.get(*f).unwrap_or(&null) != after.get(*f).unwrap_or(&null));
    carries && moved
}

pub struct Engine<'a> {
    state: &'a SyncState,
    sources: Vec<&'a dyn SyncSource>,
}

impl<'a> Engine<'a> {
    pub fn new(state: &'a SyncState, sources: Vec<&'a dyn SyncSource>) -> Self {
        Engine { state, sources }
    }

    fn source(&self, collection: &str) -> Option<&'a dyn SyncSource> {
        self.sources.iter().copied().find(|s| s.collection() == collection)
    }

    /// Pick up edits nothing told us about (made by hand, or by an older build).
    fn reconcile(&self) {
        for s in &self.sources {
            self.state.observe(s.collection(), s.live(), None);
        }
    }

    fn exists(&self, collection: &str, id: &str, batch: &Batch) -> bool {
        batch.planned.contains(&(collection.to_string(), id.to_string()))
            || self.source(collection).map(|s| s.get(id).is_some()).unwrap_or(false)
    }

    pub fn schema(&self) -> Value {
        let collections: Vec<Value> = SPECS
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "schemaVersion": s.schema_version,
                    "secretFields": s.secret_fields,
                    "localFields": s.local_fields,
                    "matchKey": s.match_key,
                    "guardFields": s.guard_fields,
                    "refs": s.refs.iter().map(|r| json!({"field": r.field, "target": r.target})).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "app": "ssh-manager",
            "syncVersion": 1,
            "deviceId": self.state.device_id(),
            "order": SPECS.iter().map(|s| s.id).collect::<Vec<_>>(),
            "collections": collections,
        })
    }

    pub fn digest(&self, only: Option<&[String]>) -> Result<Value, SyncError> {
        if let Some(names) = only {
            if let Some(bad) = names.iter().find(|n| spec_of(n).is_none()) {
                return Err(SyncError::UnknownCollection(bad.clone()));
            }
        }
        self.reconcile();
        let mut out = Map::new();
        for s in SPECS {
            if only.map(|o| o.iter().any(|n| n == s.id)).unwrap_or(true) {
                out.insert(s.id.to_string(), json!(self.state.digest(s.id)));
            }
        }
        Ok(json!({ "deviceId": self.state.device_id(), "clockMs": self.state.clock_ms(), "collections": out }))
    }

    pub fn read(&self, collection: &str, ids: &[String], secrets: bool) -> Result<Vec<WireRecord>, SyncError> {
        let spec = spec_of(collection).ok_or_else(|| SyncError::UnknownCollection(collection.into()))?;
        if ids.len() > MAX_RECORDS {
            return Err(SyncError::TooMany);
        }
        let src = self.source(collection).ok_or_else(|| SyncError::UnknownCollection(collection.into()))?;
        self.reconcile();
        let mut out = Vec::new();
        for id in ids {
            let Some(hlc) = self.state.version_of(collection, id) else { continue };
            match src.get(id) {
                Some(rec) => {
                    let mut data = strip_local(spec, &rec);
                    if !secrets {
                        strip_secrets(spec, &mut data);
                    }
                    out.push(WireRecord { collection: collection.into(), id: id.clone(), hlc: hlc.to_string(), deleted: false, data });
                }
                None => out.push(WireRecord {
                    collection: collection.into(),
                    id: id.clone(),
                    hlc: hlc.to_string(),
                    deleted: true,
                    data: Value::Null,
                }),
            }
        }
        Ok(out)
    }

    pub fn apply(&self, req: &ApplyRequest) -> Result<ApplyReport, SyncError> {
        if req.records.len() > MAX_RECORDS {
            return Err(SyncError::TooMany);
        }
        self.reconcile();
        // Targets before the records that point at them; a fixed order inside
        // a collection so two runs on the same input agree.
        let mut ordered: Vec<&WireRecord> = req.records.iter().collect();
        ordered.sort_by(|a, b| (spec_index(&a.collection), &a.id).cmp(&(spec_index(&b.collection), &b.id)));
        let mut batch = Batch::default();
        let mut results = Vec::new();
        for rec in ordered {
            let item = self.apply_one(rec, req, &mut batch);
            if matches!(item.status, "duplicate" | "held") {
                batch.held.insert((item.collection.clone(), item.id.clone()));
            }
            results.push(item);
        }
        let mut summary = BTreeMap::new();
        for r in &results {
            *summary.entry(r.status).or_insert(0) += 1;
        }
        Ok(ApplyReport { dry_run: req.dry_run, results, summary })
    }

    fn apply_one(&self, rec: &WireRecord, req: &ApplyRequest, batch: &mut Batch) -> ApplyItem {
        let (coll, id) = (rec.collection.as_str(), rec.id.as_str());
        let Some(spec) = spec_of(coll) else {
            return ApplyItem::new(coll, id, "error").reason("unknown_collection");
        };
        let Some(src) = self.source(coll) else {
            return ApplyItem::new(coll, id, "error").reason("unknown_collection");
        };
        if !valid_id(id) {
            return ApplyItem::new(coll, id, "rejected").reason("bad_id");
        }
        let Some(remote) = Hlc::parse(&rec.hlc) else {
            return ApplyItem::new(coll, id, "rejected").reason("bad_stamp");
        };
        if remote.ms > self.state.now_ms() + MAX_DRIFT_MS {
            return ApplyItem::new(coll, id, "rejected").reason("clock_skew");
        }
        if let Some(mine) = self.state.version_of(coll, id) {
            if mine == remote && req.secrets && !rec.deleted {
                if let Some(item) = self.fill_secrets(spec, src, rec, &remote, req) {
                    return item;
                }
            }
            if mine >= remote {
                return ApplyItem::new(coll, id, "skipped").reason(if mine == remote { "same" } else { "local_newer" });
            }
        }
        if rec.deleted {
            return self.apply_delete(coll, id, src, &remote, req);
        }
        let Value::Object(data) = &rec.data else {
            return ApplyItem::new(coll, id, "rejected").reason("bad_data");
        };

        let mut incoming = data.clone();
        incoming.insert("id".into(), Value::String(id.into()));
        for f in spec.local_fields {
            incoming.remove(*f);
        }
        if !req.secrets {
            for f in spec.secret_fields {
                incoming.remove(*f);
            }
        }
        let local = src.get(id);
        if let Some(Value::Object(l)) = &local {
            // A secret the sender did not send is not a secret it deleted.
            for f in spec.secret_fields {
                if !incoming.contains_key(*f) {
                    if let Some(v) = l.get(*f) {
                        incoming.insert((*f).into(), v.clone());
                    }
                }
            }
        }

        let mut warnings = Vec::new();
        for r in spec.refs {
            let Some(Value::String(target_id)) = incoming.get(r.field).cloned() else { continue };
            if target_id.is_empty() {
                continue;
            }
            let mapped = batch.id_map.get(&(r.target.to_string(), target_id.clone())).cloned().unwrap_or(target_id);
            if batch.held.contains(&(r.target.to_string(), mapped.clone())) {
                return ApplyItem::new(coll, id, "held").reason(format!("waiting_for:{}", r.field));
            }
            if !self.exists(r.target, &mapped, batch) {
                warnings.push(format!("{} points at a {} that is not on this device", r.field, r.target));
            }
            incoming.insert(r.field.into(), Value::String(mapped));
        }

        if local.is_none() {
            if let Some(mine) = self.find_match(spec, src, id, &incoming) {
                if !req.adopt_matches {
                    return ApplyItem::new(coll, id, "duplicate").reason("same_key_other_id").matched(&mine);
                }
                return self.adopt(spec, src, id, &mine, incoming, remote, req, batch, warnings);
            }
        }

        if !req.allow_risky {
            let empty = Map::new();
            let before = match &local {
                Some(Value::Object(l)) => l,
                _ => &empty,
            };
            if redirects(spec, before, &incoming) {
                return ApplyItem::new(coll, id, "held").reason("credential_target_changed");
            }
        }

        let candidate = Value::Object(incoming);
        if let Some(l) = &local {
            if content_hash(&strip_local(spec, &candidate)) == content_hash(&strip_local(spec, l)) {
                if !req.dry_run {
                    self.state.register_live(coll, id, &remote, l);
                    self.state.receive(&remote);
                }
                return ApplyItem::new(coll, id, "unchanged").warnings(warnings);
            }
        }
        let status = if local.is_some() { "updated" } else { "added" };
        if req.dry_run {
            batch.planned.insert((coll.into(), id.into()));
            return ApplyItem::new(coll, id, status).warnings(warnings);
        }
        match src.put(id, candidate, false) {
            Ok(stored) => {
                self.state.register_live(coll, id, &remote, &stored);
                self.state.receive(&remote);
                ApplyItem::new(coll, id, status).warnings(warnings)
            }
            Err(e) => ApplyItem::new(coll, id, "error").reason(e),
        }
    }

    /// The same version of a record is already here, but without the secrets it
    /// was sent without. Now that they may travel, supply the ones that are
    /// missing — never replace one that is set, and never change the stamp: this
    /// is not an edit, it is the rest of the version this device already has.
    fn fill_secrets(&self, spec: &CollectionSpec, src: &dyn SyncSource, rec: &WireRecord, remote: &Hlc, req: &ApplyRequest) -> Option<ApplyItem> {
        let Value::Object(sent) = &rec.data else { return None };
        let Some(Value::Object(mut mine)) = src.get(&rec.id) else { return None };
        let empty = |v: Option<&Value>| match v {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty(),
            Some(_) => false,
        };
        let mut filled = false;
        for f in spec.secret_fields {
            if empty(mine.get(*f)) && !empty(sent.get(*f)) {
                mine.insert((*f).into(), sent[*f].clone());
                filled = true;
            }
        }
        if !filled {
            return None;
        }
        let item = ApplyItem::new(spec.id, &rec.id, "updated").reason("secrets_filled");
        if req.dry_run {
            return Some(item);
        }
        match src.put(&rec.id, Value::Object(mine), false) {
            Ok(stored) => {
                self.state.register_live(spec.id, &rec.id, remote, &stored);
                Some(item)
            }
            Err(e) => Some(ApplyItem::new(spec.id, &rec.id, "error").reason(e)),
        }
    }

    fn apply_delete(&self, coll: &str, id: &str, src: &dyn SyncSource, remote: &Hlc, req: &ApplyRequest) -> ApplyItem {
        let live = src.get(id).is_some();
        let status = if live { "deleted" } else { "tombstoned" };
        if !req.dry_run {
            if live {
                src.remove(id, false);
            }
            self.state.register_deleted(coll, id, remote);
            self.state.receive(remote);
        }
        ApplyItem::new(coll, id, status)
    }

    /// A record already here that means the same as `incoming` but has another id.
    fn find_match(&self, spec: &CollectionSpec, src: &dyn SyncSource, id: &str, incoming: &Map<String, Value>) -> Option<String> {
        let key = match_key_of(spec, incoming)?;
        let mut hits: Vec<String> = src
            .live()
            .into_iter()
            .filter(|(other, v)| other != id && v.as_object().and_then(|o| match_key_of(spec, o)).as_ref() == Some(&key))
            .map(|(other, _)| other)
            .collect();
        hits.sort();
        hits.into_iter().next()
    }

    /// Fold two records that are the same thing under different ids into one.
    /// Both devices pick the smaller id, so they settle on the same one.
    #[allow(clippy::too_many_arguments)]
    fn adopt(
        &self,
        spec: &CollectionSpec,
        src: &dyn SyncSource,
        theirs: &str,
        mine: &str,
        mut incoming: Map<String, Value>,
        remote: Hlc,
        req: &ApplyRequest,
        batch: &mut Batch,
        warnings: Vec<String>,
    ) -> ApplyItem {
        let coll = spec.id;
        let Some(local) = src.get(mine) else {
            return ApplyItem::new(coll, theirs, "error").reason("matched_record_vanished");
        };
        let Value::Object(local_obj) = &local else {
            return ApplyItem::new(coll, theirs, "error").reason("matched_record_malformed");
        };
        if !req.allow_risky && remote > self.state.version_of(coll, mine).unwrap_or(Hlc { ms: 0, counter: 0, device: "0".into() })
            && redirects(spec, local_obj, &incoming)
        {
            return ApplyItem::new(coll, theirs, "held").reason("credential_target_changed").matched(mine);
        }
        let keep = if mine < theirs { mine } else { theirs };
        batch.id_map.insert((coll.to_string(), theirs.to_string()), keep.to_string());
        // A running tunnel is keyed by its id; renaming it underneath would orphan it.
        if keep != mine && spec.local_fields.iter().any(|f| local_obj.get(*f).and_then(Value::as_bool).unwrap_or(false)) {
            return ApplyItem::new(coll, theirs, "held").reason("local_in_use").matched(mine);
        }
        let mine_hlc = self.state.version_of(coll, mine).unwrap_or(Hlc { ms: 0, counter: 0, device: "0".into() });
        let remote_wins = remote > mine_hlc;
        let winner_hlc = if remote_wins { remote.clone() } else { mine_hlc };
        let mut winner = if remote_wins {
            for f in spec.secret_fields {
                if !incoming.contains_key(*f) {
                    if let Some(v) = local_obj.get(*f) {
                        incoming.insert((*f).into(), v.clone());
                    }
                }
            }
            Value::Object(incoming)
        } else {
            local.clone()
        };
        winner["id"] = Value::String(keep.into());
        if req.dry_run {
            batch.planned.insert((coll.into(), keep.into()));
            return ApplyItem::new(coll, theirs, "adopted").matched(mine).warnings(warnings);
        }
        if keep == mine {
            if content_hash(&strip_local(spec, &winner)) == content_hash(&strip_local(spec, &local)) {
                self.state.register_live(coll, mine, &winner_hlc, &local);
            } else {
                match src.put(mine, winner, false) {
                    Ok(stored) => self.state.register_live(coll, mine, &winner_hlc, &stored),
                    Err(e) => return ApplyItem::new(coll, theirs, "error").reason(e),
                }
            }
        } else {
            match src.put(theirs, winner, false) {
                Ok(stored) => self.state.register_live(coll, theirs, &winner_hlc, &stored),
                Err(e) => return ApplyItem::new(coll, theirs, "error").reason(e),
            }
            src.remove(mine, false);
            self.state.register_deleted(coll, mine, &self.state.tick());
            self.repoint(coll, mine, theirs);
        }
        self.state.receive(&remote);
        ApplyItem::new(coll, theirs, "adopted").matched(mine).warnings(warnings)
    }

    /// Point every record that referred to `old` at `new`. These are local
    /// edits (a record's content really changed), so they are stamped and will
    /// travel like any other.
    fn repoint(&self, target: &str, old: &str, new: &str) {
        for spec in SPECS {
            let Some(src) = self.source(spec.id) else { continue };
            for r in spec.refs.iter().filter(|r| r.target == target) {
                for (id, mut rec) in src.live() {
                    if rec.get(r.field).and_then(Value::as_str) == Some(old) {
                        rec[r.field] = Value::String(new.into());
                        let _ = src.put(&id, rec, true);
                    }
                }
            }
        }
    }
}
