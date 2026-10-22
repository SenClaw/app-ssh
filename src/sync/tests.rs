//! Two replicas of the app's data, a hand-driven clock, and the exchange the
//! SenClaw daemon performs between them: compare digests, read what differs,
//! apply on the other side.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use super::engine::{ApplyReport, ApplyRequest, Engine, WireRecord};
use super::spec::{SPECS, strip_local};
use super::state::DigestEntry;
use super::testutil::scratch;
use super::{SyncSource, SyncState};
use crate::keychain::{KeychainItem, KeychainItemType, KeychainStore};
use crate::models::{Host, HostStore, PortForwardingRule, PortForwardingStore};
use crate::snippets::{Snippet, SnippetStore};

struct Replica {
    dir: PathBuf,
    clock: Arc<AtomicU64>,
    hosts: HostStore,
    keychain: KeychainStore,
    rules: PortForwardingStore,
    snippets: SnippetStore,
    state: Arc<SyncState>,
}

impl Replica {
    fn new(tag: &str, clock: Arc<AtomicU64>) -> Replica {
        Self::open(scratch(tag), clock)
    }

    fn open(dir: PathBuf, clock: Arc<AtomicU64>) -> Replica {
        let p = |n: &str| dir.join(n).to_str().unwrap().to_string();
        let reader = clock.clone();
        let state = SyncState::open_with_clock(&dir, Box::new(move || reader.load(Ordering::SeqCst)));
        let r = Replica {
            hosts: HostStore::new(&p("hosts.json")),
            keychain: KeychainStore::new(&p("keychain.json")),
            rules: PortForwardingStore::new(&p("port_forwarding.json")),
            snippets: SnippetStore::new(&p("snippets.json")),
            dir,
            clock,
            state,
        };
        super::start(&r.dir, &r.state, &r.sources());
        r
    }

    /// Stop and start the app again over the same files.
    fn restart(self) -> Replica {
        Self::open(self.dir.clone(), self.clock.clone())
    }

    fn sources(&self) -> Vec<&dyn SyncSource> {
        vec![&self.keychain, &self.hosts, &self.rules, &self.snippets]
    }

    fn engine(&self) -> Engine<'_> {
        Engine::new(&self.state, self.sources())
    }

    fn at(&self, ms: u64) -> &Self {
        self.clock.store(ms, Ordering::SeqCst);
        self
    }

    fn host(&self, name: &str, address: &str) -> Host {
        self.hosts.add(Host {
            id: String::new(),
            name: name.into(),
            host: address.into(),
            port: 22,
            user: "root".into(),
            password: None,
            keychain_id: None,
            tags: vec![],
            extra: Default::default(),
        })
    }

    fn rule(&self, name: &str, host_id: &str, port: u16) -> PortForwardingRule {
        self.rules.add(PortForwardingRule {
            id: String::new(),
            name: name.into(),
            local_port: port,
            bind_address: "127.0.0.1".into(),
            host_id: host_id.into(),
            destination_address: "localhost".into(),
            destination_port: 80,
            active: false,
            extra: Default::default(),
        })
    }

    fn key(&self, name: &str, secret: &str) -> KeychainItem {
        self.keychain.add(KeychainItem {
            id: String::new(),
            name: name.into(),
            item_type: KeychainItemType::Password,
            value: secret.into(),
            extra: Default::default(),
        })
    }

    fn snippet(&self, name: &str) -> Snippet {
        self.snippets.add(Snippet {
            id: String::new(),
            name: name.into(),
            script: "uptime".into(),
            description: None,
            tags: vec![],
            created_at: 0,
            extra: Default::default(),
        })
    }

    /// Live content per collection, minus what stays on the machine.
    fn snapshot(&self) -> BTreeMap<&'static str, BTreeMap<String, Value>> {
        SPECS
            .iter()
            .map(|spec| {
                let src = self.sources().into_iter().find(|s| s.collection() == spec.id).unwrap();
                (spec.id, src.live().into_iter().map(|(id, v)| (id, strip_local(spec, &v))).collect())
            })
            .collect()
    }

    /// Every id this replica knows, with its stamp.
    fn stamps(&self) -> BTreeMap<&'static str, Vec<DigestEntry>> {
        let _ = self.engine().digest(None);
        SPECS.iter().map(|s| (s.id, self.state.digest(s.id))).collect()
    }
}

#[derive(Clone, Copy, Default)]
struct Opts {
    secrets: bool,
    adopt: bool,
    risky: bool,
}

/// Which ids to fetch from the peer and which to send it: the newer stamp wins,
/// and an id only one side knows goes to the other.
fn diff(local: &[DigestEntry], remote: &[DigestEntry]) -> (Vec<String>, Vec<String>) {
    let stamp = |l: &[DigestEntry]| -> BTreeMap<String, String> { l.iter().map(|e| (e.id.clone(), e.hlc.clone())).collect() };
    let (l, r) = (stamp(local), stamp(remote));
    let pull = r.iter().filter(|(id, h)| l.get(*id).map(|m| *h > m).unwrap_or(true)).map(|(id, _)| id.clone());
    let push = l.iter().filter(|(id, h)| r.get(*id).map(|m| *h > m).unwrap_or(true)).map(|(id, _)| id.clone());
    (pull.collect(), push.collect())
}

/// One sync between two replicas, both directions. Returns what each side did
/// with what it was sent.
fn sync(a: &Replica, b: &Replica, o: Opts) -> (ApplyReport, ApplyReport) {
    let (sa, sb) = (a.stamps(), b.stamps());
    let (mut into_a, mut into_b) = (Vec::new(), Vec::new());
    for spec in SPECS {
        let (pull, push) = diff(&sa[spec.id], &sb[spec.id]);
        into_a.extend(b.engine().read(spec.id, &pull, o.secrets).unwrap());
        into_b.extend(a.engine().read(spec.id, &push, o.secrets).unwrap());
    }
    let req = |records: Vec<WireRecord>| ApplyRequest {
        records,
        dry_run: false,
        secrets: o.secrets,
        adopt_matches: o.adopt,
        allow_risky: o.risky,
    };
    let ra = a.engine().apply(&req(into_a)).unwrap();
    let rb = b.engine().apply(&req(into_b)).unwrap();
    (ra, rb)
}

fn status_of(report: &ApplyReport, id: &str) -> &'static str {
    report.results.iter().find(|r| r.id == id).map(|r| r.status).unwrap_or("absent")
}

fn clock(start: u64) -> Arc<AtomicU64> {
    Arc::new(AtomicU64::new(start))
}

// ---- the basics --------------------------------------------------------

#[test]
fn a_new_record_arrives_and_a_repeat_changes_nothing() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let h = a.host("web", "web.example");
    let (_, rb) = sync(&a, &b, Opts::default());
    assert_eq!(status_of(&rb, &h.id), "added");
    assert_eq!(a.snapshot(), b.snapshot());
    // Same exchange again: nothing left to move.
    let (ra, rb) = sync(&a, &b, Opts::default());
    assert!(ra.results.is_empty() && rb.results.is_empty(), "{ra:?} {rb:?}");
    // And the batch itself is idempotent when replayed.
    let again = a.engine().read("hosts", &[h.id.clone()], false).unwrap();
    let report = b.engine().apply(&ApplyRequest { records: again, ..Default::default() }).unwrap();
    assert_eq!(status_of(&report, &h.id), "skipped");
}

#[test]
fn the_newer_edit_wins_on_both_sides_whoever_syncs_first() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let h = a.host("web", "web.example");
    sync(&a, &b, Opts::default());
    let mut older = a.hosts.get(&h.id).unwrap();
    older.name = "from-a".into();
    a.at(2_000).hosts.update(&h.id, older);
    let mut newer = b.hosts.get(&h.id).unwrap();
    newer.name = "from-b".into();
    b.at(3_000).hosts.update(&h.id, newer);
    sync(&b, &a, Opts::default());
    assert_eq!(a.hosts.get(&h.id).unwrap().name, "from-b");
    assert_eq!(b.hosts.get(&h.id).unwrap().name, "from-b");
    assert_eq!(a.stamps(), b.stamps());
}

#[test]
fn a_deletion_travels_and_an_edit_made_after_it_brings_the_record_back() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let h = a.host("web", "web.example");
    sync(&a, &b, Opts::default());

    // Delete first, edit later elsewhere: the edit is the last word.
    a.at(2_000).hosts.delete(&h.id);
    let mut edited = b.hosts.get(&h.id).unwrap();
    edited.name = "kept".into();
    b.at(3_000).hosts.update(&h.id, edited);
    sync(&a, &b, Opts::default());
    assert_eq!(a.hosts.get(&h.id).unwrap().name, "kept");
    assert_eq!(a.snapshot(), b.snapshot());

    // Edit first, delete later: the delete is.
    let mut edited = a.hosts.get(&h.id).unwrap();
    edited.name = "again".into();
    a.at(4_000).hosts.update(&h.id, edited);
    b.at(5_000).hosts.delete(&h.id);
    let (_, rb) = sync(&a, &b, Opts::default());
    assert!(b.hosts.get(&h.id).is_none() && a.hosts.get(&h.id).is_none(), "{rb:?}");
    assert_eq!(a.stamps(), b.stamps());
}

#[test]
fn a_device_that_was_away_catches_up_in_one_exchange() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let keep = a.host("keep", "keep.example");
    let drop = a.host("drop", "drop.example");
    sync(&a, &b, Opts::default());
    for i in 0..5 {
        a.at(2_000 + i).host(&format!("new{i}"), &format!("n{i}.example"));
    }
    a.at(3_000).hosts.delete(&drop.id);
    let mut e = a.hosts.get(&keep.id).unwrap();
    e.tags = vec!["prod".into()];
    a.at(3_100).hosts.update(&keep.id, e);
    sync(&a, &b, Opts::default());
    assert_eq!(a.snapshot(), b.snapshot());
    assert_eq!(b.hosts.get_all().len(), 6);
}

// ---- what must not travel ----------------------------------------------

#[test]
fn passwords_stay_home_and_the_ones_already_here_are_kept() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let mut h = a.host("web", "web.example");
    h.password = Some("a-secret".into());
    a.hosts.update(&h.id.clone(), h.clone());

    // The wire carries no password unless asked for.
    let wire = a.engine().read("hosts", &[h.id.clone()], false).unwrap();
    assert!(wire[0].data.get("password").is_none());
    let with = a.engine().read("hosts", &[h.id.clone()], true).unwrap();
    assert_eq!(with[0].data["password"], "a-secret");

    sync(&a, &b, Opts::default());
    assert_eq!(b.hosts.get(&h.id).unwrap().password, None, "arrives without a secret");

    // B types its own; A renames the host; B keeps its password.
    let mut mine = b.hosts.get(&h.id).unwrap();
    mine.password = Some("b-secret".into());
    b.at(2_000).hosts.update(&h.id, mine);
    let mut renamed = a.hosts.get(&h.id).unwrap();
    renamed.name = "web-2".into();
    a.at(3_000).hosts.update(&h.id, renamed);
    sync(&a, &b, Opts::default());
    let on_b = b.hosts.get(&h.id).unwrap();
    assert_eq!((on_b.name.as_str(), on_b.password.as_deref()), ("web-2", Some("b-secret")));
    assert_eq!(a.hosts.get(&h.id).unwrap().password.as_deref(), Some("a-secret"));
}

#[test]
fn secrets_are_dropped_on_receipt_even_if_the_sender_included_them() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let k = a.key("deploy key", "hunter2");
    let records = a.engine().read("keychain", &[k.id.clone()], true).unwrap();
    assert_eq!(records[0].data["value"], "hunter2");
    b.engine().apply(&ApplyRequest { records, secrets: false, ..Default::default() }).unwrap();
    assert_eq!(b.keychain.get(&k.id).unwrap().value, "", "a placeholder, to be filled in on this machine");
}

#[test]
fn secrets_allowed_later_fill_what_was_left_out_without_a_new_edit() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let k = a.key("deploy key", "hunter2");
    let mut h = a.host("web", "web.example");
    h.password = Some("a-web".into());
    a.hosts.update(&h.id.clone(), h.clone());
    sync(&a, &b, Opts::default());
    assert_eq!(b.keychain.get(&k.id).unwrap().value, "");
    let stamps = b.stamps();

    // The stamps already agree, so the exchange has nothing to send; the whole
    // record is read again once secrets are allowed.
    let mut all = a.engine().read("keychain", &[k.id.clone()], true).unwrap();
    all.extend(a.engine().read("hosts", &[h.id.clone()], true).unwrap());
    let report = b.engine().apply(&ApplyRequest { records: all.clone(), secrets: true, ..Default::default() }).unwrap();
    assert_eq!(status_of(&report, &k.id), "updated");
    assert_eq!(b.keychain.get(&k.id).unwrap().value, "hunter2");
    assert_eq!(b.hosts.get(&h.id).unwrap().password.as_deref(), Some("a-web"));
    assert_eq!(b.stamps(), stamps, "not an edit: nothing new to sync back");

    // A secret that is already set is never replaced this way.
    let mut mine = b.hosts.get(&h.id).unwrap();
    mine.password = Some("b-own".into());
    b.at(2_000).hosts.update(&h.id, mine);
    let again = b.engine().apply(&ApplyRequest { records: all, secrets: true, ..Default::default() }).unwrap();
    assert_eq!(b.hosts.get(&h.id).unwrap().password.as_deref(), Some("b-own"), "{again:?}");
}

#[test]
fn with_secrets_on_both_ends_they_move() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let k = a.key("deploy key", "hunter2");
    sync(&a, &b, Opts { secrets: true, ..Default::default() });
    assert_eq!(b.keychain.get(&k.id).unwrap().value, "hunter2");
}

#[test]
fn an_open_tunnel_never_travels_and_is_never_switched_by_a_peer() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let h = a.host("web", "web.example");
    let r = a.rule("db", &h.id, 5432);
    a.rules.set_active(&r.id, true);
    sync(&a, &b, Opts::default());
    assert!(!b.rules.get(&r.id).unwrap().active, "arrives off");

    b.rules.set_active(&r.id, true);
    let stamp = b.stamps();
    let mut edited = a.rules.get(&r.id).unwrap();
    edited.local_port = 5433;
    a.at(2_000).rules.update(&r.id, edited);
    sync(&a, &b, Opts::default());
    let on_b = b.rules.get(&r.id).unwrap();
    assert_eq!((on_b.local_port, on_b.active), (5433, true), "edit applied, local state kept");
    assert_ne!(stamp["port_forwarding"], b.stamps()["port_forwarding"]);

    // Opening or closing a tunnel is not an edit to sync.
    let before = a.stamps();
    a.rules.set_active(&r.id, false);
    assert_eq!(before, a.stamps());
}

#[test]
fn a_reference_to_something_not_here_yet_is_kept_and_flagged() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let h = a.host("web", "web.example");
    let r = a.rule("db", &h.id, 5432);
    let only_rule = a.engine().read("port_forwarding", &[r.id.clone()], false).unwrap();
    let report = b.engine().apply(&ApplyRequest { records: only_rule, ..Default::default() }).unwrap();
    assert_eq!(status_of(&report, &r.id), "added");
    assert!(report.results[0].warnings[0].contains("host_id"), "{:?}", report.results[0]);
    // The host arrives later and the rule is whole again.
    sync(&a, &b, Opts::default());
    assert_eq!(b.rules.get(&r.id).unwrap().host_id, h.id);
    assert_eq!(a.snapshot(), b.snapshot());
}

// ---- the credential-redirect guard ------------------------------------

#[test]
fn moving_a_host_that_holds_a_password_waits_for_a_person() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let h = a.host("web", "web.example");
    sync(&a, &b, Opts::default());
    let mut mine = b.hosts.get(&h.id).unwrap();
    mine.password = Some("b-secret".into());
    b.at(2_000).hosts.update(&h.id, mine);

    let mut moved = a.hosts.get(&h.id).unwrap();
    moved.host = "evil.example".into();
    a.at(3_000).hosts.update(&h.id, moved);
    let (_, rb) = sync(&a, &b, Opts::default());
    assert_eq!(status_of(&rb, &h.id), "held");
    assert_eq!(b.hosts.get(&h.id).unwrap().host, "web.example");

    // A rename alone is not a redirect.
    let mut renamed = a.hosts.get(&h.id).unwrap();
    renamed.host = "web.example".into();
    renamed.name = "web-2".into();
    a.at(4_000).hosts.update(&h.id, renamed);
    let (_, rb) = sync(&a, &b, Opts::default());
    assert_eq!(status_of(&rb, &h.id), "updated");

    // Once a person approves, it goes through.
    let mut moved = a.hosts.get(&h.id).unwrap();
    moved.host = "new.example".into();
    a.at(5_000).hosts.update(&h.id, moved);
    let (_, rb) = sync(&a, &b, Opts { risky: true, ..Default::default() });
    assert_eq!(status_of(&rb, &h.id), "updated");
    assert_eq!(b.hosts.get(&h.id).unwrap().host, "new.example");
}

#[test]
fn a_host_that_arrives_wired_to_a_keychain_item_waits_for_a_person() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    // B has a credential of its own; a peer that learned its id points a new host at it.
    let mine = b.key("prod key", "b-private-key");
    let wire = |address: &str, keychain: &str, stamp: &str| WireRecord {
        collection: "hosts".into(),
        id: "aaaaaaaa-0000-0000-0000-000000000001".into(),
        hlc: stamp.into(),
        deleted: false,
        data: json!({"name": "x", "host": address, "port": 22, "user": "root", "tags": [], "keychain_id": keychain}),
    };
    let req = |r: WireRecord, risky: bool| ApplyRequest { records: vec![r], allow_risky: risky, ..Default::default() };
    let held = b.engine().apply(&req(wire("evil.example", &mine.id, "0000000001000-00000-peer"), false)).unwrap();
    assert_eq!(held.results[0].status, "held");
    assert!(b.hosts.get_all().is_empty());
    let ok = b.engine().apply(&req(wire("evil.example", &mine.id, "0000000001000-00000-peer"), true)).unwrap();
    assert_eq!(ok.results[0].status, "added");

    // A plain host cannot be moved and handed a credential in one step either.
    let plain = a.host("plain", "plain.example");
    sync(&a, &b, Opts::default());
    let mut sneaky = a.hosts.get(&plain.id).unwrap();
    sneaky.host = "evil.example".into();
    sneaky.keychain_id = Some(mine.id.clone());
    a.at(5_000).hosts.update(&plain.id, sneaky);
    let (_, rb) = sync(&a, &b, Opts::default());
    assert_eq!(status_of(&rb, &plain.id), "held");
    assert_eq!(b.hosts.get(&plain.id).unwrap().host, "plain.example");
}

// ---- the same thing created twice -------------------------------------

#[test]
fn the_same_host_under_two_ids_is_reported_then_folded_into_one() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let on_a = a.host("web", "web.example");
    let on_b = b.at(1_100).host("web (b)", "web.example");
    a.rule("db", &on_a.id, 5432);
    b.rule("db", &on_b.id, 5432);

    // Not asked to merge: the duplicate is named, nothing is added.
    let (ra, _) = sync(&a, &b, Opts::default());
    let dup = ra.results.iter().find(|r| r.id == on_b.id).unwrap();
    assert_eq!((dup.status, dup.matched.as_deref()), ("duplicate", Some(on_a.id.as_str())));
    assert_eq!(a.hosts.get_all().len(), 1);
    assert_eq!(a.rules.get_all().len(), 1, "the tunnel that points at the duplicate waits with it");

    for _ in 0..3 {
        sync(&a, &b, Opts { adopt: true, ..Default::default() });
    }
    assert_eq!(a.snapshot(), b.snapshot());
    assert_eq!(a.hosts.get_all().len(), 1, "one host left on each side");
    assert_eq!(a.rules.get_all().len(), 1, "and one tunnel");
    let kept = std::cmp::min(on_a.id.clone(), on_b.id.clone());
    assert_eq!(a.hosts.get_all()[0].id, kept);
    assert_eq!(a.rules.get_all()[0].host_id, kept, "the tunnel follows the host");
    assert_eq!(b.rules.get_all()[0].host_id, kept);
    assert_eq!(a.stamps(), b.stamps());
}

#[test]
fn a_tunnel_that_is_open_is_not_renamed_under_itself() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let h = a.host("web", "web.example");
    sync(&a, &b, Opts::default());
    // The same tunnel made on both sides: same key, different ids.
    let ra = a.rule("db", &h.id, 5432);
    let rb = b.rule("db", &h.id, 5432);
    // Both sides settle on the smaller id, so the one holding the larger id
    // would have to rename. Open there, it must not.
    let (owner, big) = if ra.id > rb.id { (&a, &ra) } else { (&b, &rb) };
    owner.rules.set_active(&big.id, true);
    let (ra_report, rb_report) = sync(&a, &b, Opts { adopt: true, ..Default::default() });
    let held = [&ra_report, &rb_report].iter().any(|r| r.results.iter().any(|i| i.status == "held" && i.reason.as_deref() == Some("local_in_use")));
    assert!(held, "{ra_report:?} {rb_report:?}");
    let still = owner.rules.get(&big.id).unwrap();
    assert!(still.active, "the open tunnel keeps its id and state");
}

// ---- restarts, hand edits, first run -----------------------------------

#[test]
fn stamps_survive_a_restart_and_a_hand_edit_is_noticed() {
    let c = clock(1_000);
    let a = Replica::new("a", c.clone());
    let keep = a.host("keep", "keep.example");
    let gone = a.host("gone", "gone.example");
    let before = a.stamps();
    let a = a.restart();
    assert_eq!(a.stamps(), before);

    // Someone edits the file while the app is stopped.
    let dir = a.dir.clone();
    drop(a);
    let path = dir.join("hosts.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v[&keep.id]["name"] = json!("renamed by hand");
    v.as_object_mut().unwrap().remove(&gone.id);
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    c.store(9_000, Ordering::SeqCst);
    let a = Replica::open(dir, c.clone());
    let after = a.stamps();
    let find = |id: &str| after["hosts"].iter().find(|e| e.id == id).unwrap().clone();
    assert!(find(&keep.id).hlc > before["hosts"].iter().find(|e| e.id == keep.id).unwrap().hlc);
    assert!(find(&gone.id).deleted);
}

#[test]
fn first_run_backs_up_tightens_and_stamps_with_the_file_time() {
    let dir = scratch("first");
    let pre = r#"{"h1":{"id":"h1","name":"old","host":"o.example","port":22,"user":"u","password":"pw","keychain_id":null,"tags":[]}}"#;
    std::fs::write(dir.join("hosts.json"), pre).unwrap();
    let r = Replica::open(dir.clone(), clock(5_000_000_000_000));
    assert_eq!(std::fs::read_to_string(dir.join("hosts.json.pre-sync.bak")).unwrap(), pre);
    let stamp = r.state.version_of("hosts", "h1").unwrap();
    assert!(stamp.ms < 5_000_000_000_000, "stamped with when the file last changed, not now");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |n: &str| std::fs::metadata(dir.join(n)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("hosts.json"), 0o600);
        assert_eq!(mode("hosts.json.pre-sync.bak"), 0o600);
        assert_eq!(mode("sync.json"), 0o600);
    }
    // The collection files keep the shape an older build reads.
    r.host("two", "two.example");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("hosts.json")).unwrap()).unwrap();
    assert!(v.as_object().unwrap().values().all(|h| h.get("name").is_some()));
    let again = r.restart();
    assert!(!again.state.is_fresh());
    assert_eq!(again.state.version_of("hosts", "h1").unwrap(), stamp);
}

// ---- what a peer is not allowed to do ---------------------------------

#[test]
fn bad_ids_future_stamps_and_unknown_collections_are_refused() {
    let c = clock(1_000_000);
    let a = Replica::new("a", c.clone());
    let rec = |collection: &str, id: &str, hlc: &str| WireRecord {
        collection: collection.into(),
        id: id.into(),
        hlc: hlc.into(),
        deleted: false,
        data: json!({"name": "x", "host": "h", "port": 22, "user": "u", "tags": []}),
    };
    let ok_stamp = "0000001000500-00000-peer";
    let report = a
        .engine()
        .apply(&ApplyRequest {
            records: vec![
                rec("hosts", "../etc", ok_stamp),
                rec("hosts", "fine", "0009999999999-00000-peer"),
                rec("hosts", "fine2", "not a stamp"),
                rec("settings", "s", ok_stamp),
                rec("hosts", "good", ok_stamp),
            ],
            ..Default::default()
        })
        .unwrap();
    let reasons: Vec<_> = report.results.iter().map(|r| (r.id.as_str(), r.status, r.reason.as_deref())).collect();
    assert!(reasons.contains(&("../etc", "rejected", Some("bad_id"))), "{reasons:?}");
    assert!(reasons.contains(&("fine", "rejected", Some("clock_skew"))), "{reasons:?}");
    assert!(reasons.contains(&("fine2", "rejected", Some("bad_stamp"))), "{reasons:?}");
    assert!(reasons.contains(&("s", "error", Some("unknown_collection"))), "{reasons:?}");
    assert!(reasons.contains(&("good", "added", None)), "{reasons:?}");
    assert_eq!(a.hosts.get_all().len(), 1);
    assert!(a.engine().read("settings", &[], false).is_err());
    assert!(a.engine().digest(Some(&["settings".to_string()])).is_err());
}

#[test]
fn a_dry_run_reports_and_changes_nothing() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let h = a.host("web", "web.example");
    a.rule("db", &h.id, 5432);
    let before = (b.snapshot(), b.stamps());
    let mut records = Vec::new();
    for spec in SPECS {
        let ids: Vec<String> = a.state.digest(spec.id).into_iter().map(|e| e.id).collect();
        records.extend(a.engine().read(spec.id, &ids, false).unwrap());
    }
    let report = b.engine().apply(&ApplyRequest { records, dry_run: true, ..Default::default() }).unwrap();
    assert!(report.dry_run);
    assert_eq!(report.summary.get("added"), Some(&2));
    assert!(report.results.iter().all(|r| r.warnings.is_empty()), "the host is planned, so the tunnel's reference holds");
    assert_eq!((b.snapshot(), b.stamps()), before);
}

#[test]
fn fields_this_build_does_not_know_pass_through_intact() {
    let c = clock(1_000);
    let (a, b) = (Replica::new("a", c.clone()), Replica::new("b", c.clone()));
    let id = "11111111-2222-3333-4444-555555555555";
    let records = vec![WireRecord {
        collection: "hosts".into(),
        id: id.into(),
        hlc: "0000000001000-00000-newer".into(),
        deleted: false,
        data: json!({"name": "web", "host": "h", "port": 22, "user": "u", "tags": [], "jump_host": "bastion", "weight": 3}),
    }];
    a.engine().apply(&ApplyRequest { records: records.clone(), ..Default::default() }).unwrap();
    // It is relayed to a third device with the field still there.
    let relayed = a.engine().read("hosts", &[id.into()], false).unwrap();
    assert_eq!(relayed[0].data["jump_host"], "bastion");
    assert_eq!(relayed[0].data["weight"], 3);
    b.engine().apply(&ApplyRequest { records: relayed, ..Default::default() }).unwrap();
    assert_eq!(b.hosts.get(id).unwrap().extra["jump_host"], "bastion");
}

// ---- the property that matters ----------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Random edits on two devices, exchanges at random moments in random
/// directions, restarts now and then: once the dust settles both devices hold
/// the same records under the same stamps.
fn converges(seed: u64) {
    let c = clock(1_000_000);
    let mut devices = [Replica::new("ra", c.clone()), Replica::new("rb", c.clone())];
    let mut rng = Rng(seed * 2654435761 + 1);
    let mut serial = 0u32;
    for _ in 0..80 {
        c.fetch_add(1 + rng.below(40) as u64, Ordering::SeqCst);
        let d = &devices[rng.below(2)];
        serial += 1;
        match rng.below(10) {
            0 | 1 => {
                d.host(&format!("h{serial}"), &format!("h{serial}.example"));
            }
            2 => {
                d.key(&format!("k{serial}"), &format!("s{serial}"));
            }
            3 => {
                d.snippet(&format!("s{serial}"));
            }
            4 => {
                if let Some(h) = d.hosts.get_all().first() {
                    d.rule(&format!("r{serial}"), &h.id, 1000 + serial as u16);
                }
            }
            5 | 6 => {
                let all = d.hosts.get_all();
                if !all.is_empty() {
                    let mut h = all[rng.below(all.len())].clone();
                    h.name = format!("renamed{serial}");
                    h.tags = vec![format!("t{serial}")];
                    d.hosts.update(&h.id.clone(), h);
                }
            }
            7 => {
                let all = d.hosts.get_all();
                if !all.is_empty() {
                    d.hosts.delete(&all[rng.below(all.len())].id);
                }
            }
            8 => {
                let all = d.rules.get_all();
                if !all.is_empty() {
                    d.rules.delete(&all[rng.below(all.len())].id);
                }
            }
            _ => {
                let (from, to) = if rng.below(2) == 0 { (0, 1) } else { (1, 0) };
                sync(&devices[from], &devices[to], Opts { secrets: true, ..Default::default() });
            }
        }
        if rng.below(25) == 0 {
            let i = rng.below(2);
            let old = std::mem::replace(&mut devices[i], Replica::new("tmp", c.clone()));
            devices[i] = old.restart();
        }
    }
    for _ in 0..3 {
        sync(&devices[0], &devices[1], Opts { secrets: true, ..Default::default() });
    }
    assert_eq!(devices[0].snapshot(), devices[1].snapshot(), "seed {seed}: content differs");
    assert_eq!(devices[0].stamps(), devices[1].stamps(), "seed {seed}: stamps differ");
}

#[test]
fn two_devices_always_end_up_identical() {
    for seed in 1..=60 {
        converges(seed);
    }
}

#[test]
fn the_manifest_declares_exactly_what_the_code_enforces() {
    let manifest: Value = serde_json::from_str(include_str!("../../senclaw-manifest.json")).unwrap();
    let schema = Replica::new("schema", clock(1)).engine().schema();
    assert_eq!(manifest["sync"]["collections"], schema["collections"]);
    assert_eq!(manifest["sync"]["order"], schema["order"]);
    assert_eq!(manifest["sync"]["version"], schema["syncVersion"]);
    assert_eq!(manifest["sync"]["endpoint"], "/api/sync");
}
