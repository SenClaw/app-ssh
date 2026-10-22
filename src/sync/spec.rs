//! What each collection is, as far as syncing goes: which fields are secret,
//! which belong to this machine, how two records are recognised as the same
//! thing, and which fields must not change silently.
//!
//! This table is the app's side of the sync contract and is served as-is by
//! `GET /api/sync/schema`; the SenClaw daemon enforces the same lists on the
//! wire, so a mistake here is caught twice rather than trusted once.

use serde_json::{Map, Value};

/// A field holding the id of a record in another collection. It may dangle —
/// the app already shows a tunnel whose host is gone — so a record is applied
/// even when its target is not here yet, and the reference resolves when the
/// target arrives, in this sync or a later one.
pub struct RefSpec {
    pub field: &'static str,
    pub target: &'static str,
}

pub struct CollectionSpec {
    pub id: &'static str,
    pub schema_version: u32,
    /// Never leave this machine unless the caller says secrets may travel.
    pub secret_fields: &'static [&'static str],
    /// Never leave this machine and never overwritten by a peer.
    pub local_fields: &'static [&'static str],
    /// Fields that together say "this is the same thing" for two records that
    /// were created independently and so carry different ids.
    pub match_key: &'static [&'static str],
    pub refs: &'static [RefSpec],
    /// Changing any of these on a record that holds credentials redirects
    /// where those credentials get sent, so such a change is held for a person.
    pub guard_fields: &'static [&'static str],
    pub has_credentials: fn(&Map<String, Value>) -> bool,
}

fn filled(v: Option<&Value>) -> bool {
    match v {
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

fn host_has_credentials(r: &Map<String, Value>) -> bool {
    filled(r.get("password")) || filled(r.get("keychain_id"))
}

fn never(_: &Map<String, Value>) -> bool {
    false
}

/// In dependency order: a record's targets come before it.
pub const SPECS: &[CollectionSpec] = &[
    CollectionSpec {
        id: "keychain",
        schema_version: 1,
        secret_fields: &["value"],
        local_fields: &[],
        match_key: &["name", "item_type"],
        refs: &[],
        guard_fields: &[],
        has_credentials: never,
    },
    CollectionSpec {
        id: "hosts",
        schema_version: 1,
        secret_fields: &["password"],
        local_fields: &[],
        match_key: &["host", "port", "user"],
        refs: &[RefSpec { field: "keychain_id", target: "keychain" }],
        guard_fields: &["host", "port", "user", "keychain_id"],
        has_credentials: host_has_credentials,
    },
    CollectionSpec {
        id: "port_forwarding",
        schema_version: 1,
        secret_fields: &[],
        // Whether a tunnel is open is a fact about this machine, and the app
        // reopens active tunnels at startup: a peer must never switch one on.
        local_fields: &["active"],
        match_key: &["host_id", "local_port", "destination_address", "destination_port"],
        refs: &[RefSpec { field: "host_id", target: "hosts" }],
        guard_fields: &[],
        has_credentials: never,
    },
    CollectionSpec {
        id: "snippets",
        schema_version: 1,
        secret_fields: &[],
        local_fields: &[],
        match_key: &["name"],
        refs: &[],
        guard_fields: &[],
        has_credentials: never,
    },
];

pub fn spec_of(collection: &str) -> Option<&'static CollectionSpec> {
    SPECS.iter().find(|s| s.id == collection)
}

pub fn spec_index(collection: &str) -> usize {
    SPECS.iter().position(|s| s.id == collection).unwrap_or(usize::MAX)
}

/// The record minus what stays on this machine: what is hashed and sent.
pub fn strip_local(spec: &CollectionSpec, record: &Value) -> Value {
    let mut out = record.clone();
    if let Value::Object(m) = &mut out {
        for f in spec.local_fields {
            m.remove(*f);
        }
    }
    out
}

pub fn strip_secrets(spec: &CollectionSpec, record: &mut Value) {
    if let Value::Object(m) = record {
        for f in spec.secret_fields {
            m.remove(*f);
        }
    }
}

/// The match-key values of a record, or `None` when any is missing or empty
/// (an empty name is not evidence of anything).
pub fn match_key_of(spec: &CollectionSpec, record: &Map<String, Value>) -> Option<Vec<String>> {
    if spec.match_key.is_empty() {
        return None;
    }
    spec.match_key
        .iter()
        .map(|f| match record.get(*f) {
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        })
        .collect()
}

fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                canonical(&m[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// A stable fingerprint of a record's content. FNV-1a over key-sorted JSON:
/// stored on disk, so it must not change with the toolchain (std's hashers may).
pub fn content_hash(v: &Value) -> String {
    let mut s = String::new();
    canonical(v, &mut s);
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Ids end up in map keys, URLs (`/hosts/:id`) and references, so only the
/// characters a generated id uses are accepted from a peer.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hash_ignores_key_order_but_not_content() {
        let a = json!({"b": 1, "a": [1, {"y": 2, "x": 3}]});
        let b = json!({"a": [1, {"x": 3, "y": 2}], "b": 1});
        assert_eq!(content_hash(&a), content_hash(&b));
        assert_ne!(content_hash(&a), content_hash(&json!({"b": 2, "a": [1, {"x": 3, "y": 2}]})));
    }

    #[test]
    fn hash_is_pinned() {
        // Stored in sync.json: a change here would make every record look edited.
        assert_eq!(content_hash(&json!({"a": 1})), "9c3e82dd6fcae8b1");
    }

    #[test]
    fn local_and_secret_fields_are_cut_by_the_spec() {
        let rule = spec_of("port_forwarding").unwrap();
        let v = strip_local(rule, &json!({"id": "r", "active": true, "local_port": 80}));
        assert_eq!(v, json!({"id": "r", "local_port": 80}));
        let host = spec_of("hosts").unwrap();
        let mut h = json!({"id": "h", "password": "x", "user": "u"});
        strip_secrets(host, &mut h);
        assert_eq!(h, json!({"id": "h", "user": "u"}));
    }

    #[test]
    fn a_match_key_needs_every_part() {
        let host = spec_of("hosts").unwrap();
        let full = json!({"host": " a.example ", "port": 22, "user": "root"});
        assert_eq!(match_key_of(host, full.as_object().unwrap()), Some(vec!["a.example".into(), "22".into(), "root".into()]));
        let blank = json!({"host": "a.example", "port": 22, "user": " "});
        assert_eq!(match_key_of(host, blank.as_object().unwrap()), None);
    }

    #[test]
    fn ids_from_a_peer_are_restricted() {
        assert!(valid_id("3f2b9c1e-0a4d-4f6e-9d1a-2b7c8e9f0a1b"));
        for bad in ["", "a/b", "a b", "../x", &"x".repeat(65)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }
}
