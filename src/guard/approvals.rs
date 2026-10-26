//! Commands waiting for the admin, and what became of them.
//!
//! A command the guard would not clear is parked here with where it was meant
//! to run. The admin approves (the app then runs it itself, on that target) or
//! rejects it; the agent reads the outcome with `ssh_command_status`. Nothing
//! here is written to disk: after a restart the queue is empty and the agent
//! is told the id is unknown, which is safer than running a stale command.

use serde::Serialize;
use std::sync::Mutex;

use super::Check;
use super::learned::now_ms;
use crate::remote::Target;

/// A request nobody answered within this is closed as expired.
pub const PENDING_TTL_MS: u64 = 60 * 60 * 1000;
/// Finished requests kept for the history list.
const KEEP: usize = 200;
/// Output kept per request; the agent gets the same text.
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Running,
    Done,
    Failed,
    Rejected,
    Expired,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Rejected => "rejected",
            Status::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Approval {
    pub id: String,
    pub command: String,
    /// Where it runs, for people (`web (root@10.0.0.5:22)`).
    pub target: String,
    #[serde(skip)]
    pub run_on: Target,
    /// `mcp` (the agent) or `rest` (`POST /api/execute`).
    pub origin: String,
    pub status: Status,
    pub checks: Vec<Check>,
    pub requested_at: u64,
    pub decided_at: Option<u64>,
    pub finished_at: Option<u64>,
    /// Approved with "remember": the exact command was added to the learned list.
    pub remembered: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    /// The admin's words when rejecting.
    pub note: Option<String>,
}

#[derive(Default)]
pub struct ApprovalStore {
    items: Mutex<Vec<Approval>>,
}

fn cap(mut s: String) -> String {
    if s.len() > MAX_OUTPUT {
        let mut end = MAX_OUTPUT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push_str("\n… (output cut at 64 KB)");
    }
    s
}

impl ApprovalStore {
    fn expire(items: &mut [Approval], now: u64) {
        for a in items.iter_mut() {
            if a.status == Status::Pending && now.saturating_sub(a.requested_at) > PENDING_TTL_MS {
                a.status = Status::Expired;
                a.finished_at = Some(now);
            }
        }
    }

    pub fn create(&self, command: &str, target: &str, run_on: Target, origin: &str, checks: Vec<Check>) -> Approval {
        let a = Approval {
            id: uuid::Uuid::new_v4().to_string(),
            command: command.to_string(),
            target: target.to_string(),
            run_on,
            origin: origin.to_string(),
            status: Status::Pending,
            checks,
            requested_at: now_ms(),
            decided_at: None,
            finished_at: None,
            remembered: false,
            output: None,
            error: None,
            note: None,
        };
        let mut items = self.items.lock().unwrap();
        items.push(a.clone());
        // Drop the oldest finished ones; pending requests are never dropped.
        while items.len() > KEEP {
            match items.iter().position(|x| x.status != Status::Pending && x.status != Status::Running) {
                Some(i) => {
                    items.remove(i);
                }
                None => break,
            }
        }
        a
    }

    /// Newest first.
    pub fn list(&self) -> Vec<Approval> {
        let mut items = self.items.lock().unwrap();
        Self::expire(&mut items, now_ms());
        let mut v = items.clone();
        v.reverse();
        v
    }

    pub fn get(&self, id: &str) -> Option<Approval> {
        let mut items = self.items.lock().unwrap();
        Self::expire(&mut items, now_ms());
        items.iter().find(|a| a.id == id).cloned()
    }

    pub fn pending_count(&self) -> usize {
        self.list().iter().filter(|a| a.status == Status::Pending).count()
    }

    fn update<T>(&self, id: &str, f: impl FnOnce(&mut Approval) -> Result<T, String>) -> Result<T, String> {
        let mut items = self.items.lock().unwrap();
        Self::expire(&mut items, now_ms());
        let a = items
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("no request with id {id}"))?;
        f(a)
    }

    fn require_pending(a: &Approval) -> Result<(), String> {
        if a.status == Status::Pending {
            Ok(())
        } else {
            Err(format!("this request is already {}", a.status.as_str()))
        }
    }

    /// Pending → running. Only one caller can win this, so a double click
    /// cannot run the command twice.
    pub fn approve(&self, id: &str, remember: bool) -> Result<Approval, String> {
        self.update(id, |a| {
            Self::require_pending(a)?;
            a.status = Status::Running;
            a.decided_at = Some(now_ms());
            a.remembered = remember;
            Ok(a.clone())
        })
    }

    pub fn reject(&self, id: &str, note: Option<String>) -> Result<Approval, String> {
        self.update(id, |a| {
            Self::require_pending(a)?;
            a.status = Status::Rejected;
            let now = now_ms();
            a.decided_at = Some(now);
            a.finished_at = Some(now);
            a.note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
            Ok(a.clone())
        })
    }

    /// Running → done / failed. Any other state is left alone.
    pub fn finish(&self, id: &str, result: Result<String, String>) -> Option<Approval> {
        self.update(id, |a| {
            if a.status != Status::Running {
                return Err(format!("this request is {}, not running", a.status.as_str()));
            }
            a.finished_at = Some(now_ms());
            match result {
                Ok(out) => {
                    a.status = Status::Done;
                    a.output = Some(cap(out));
                }
                Err(e) => {
                    a.status = Status::Failed;
                    a.error = Some(e);
                }
            }
            Ok(a.clone())
        })
        .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target::Connection { id: "c1".into() }
    }

    #[test]
    fn a_request_is_decided_once() {
        let s = ApprovalStore::default();
        let a = s.create("reboot", "web", target(), "mcp", vec![]);
        assert_eq!(s.pending_count(), 1);
        assert_eq!(s.approve(&a.id, true).unwrap().status, Status::Running);
        assert!(s.approve(&a.id, false).unwrap_err().contains("running"));
        assert!(s.reject(&a.id, None).is_err());
        let done = s.finish(&a.id, Ok("ok".into())).unwrap();
        assert_eq!(done.status, Status::Done);
        assert!(done.remembered);
        assert_eq!(s.pending_count(), 0);

        let b = s.create("rm -rf /x", "web", target(), "mcp", vec![]);
        let r = s.reject(&b.id, Some("  no  ".into())).unwrap();
        assert_eq!((r.status, r.note.as_deref()), (Status::Rejected, Some("no")));
        assert!(s.approve(&b.id, false).is_err());
        assert!(s.get("nope").is_none());
        // A late result cannot turn a rejected request into a finished one.
        assert!(s.finish(&b.id, Ok("x".into())).is_none());
        assert_eq!(s.get(&b.id).unwrap().status, Status::Rejected);
        assert_eq!(s.list()[0].id, b.id);
    }

    #[test]
    fn an_old_request_expires_and_cannot_be_approved() {
        let s = ApprovalStore::default();
        let a = s.create("reboot", "web", target(), "mcp", vec![]);
        s.items.lock().unwrap()[0].requested_at -= PENDING_TTL_MS + 1;
        assert_eq!(s.get(&a.id).unwrap().status, Status::Expired);
        assert!(s.approve(&a.id, false).unwrap_err().contains("expired"));
    }

    #[test]
    fn output_is_capped_on_a_char_boundary() {
        let s = ApprovalStore::default();
        let a = s.create("cat big", "web", target(), "mcp", vec![]);
        s.approve(&a.id, false).unwrap();
        let out = "é".repeat(MAX_OUTPUT);
        let done = s.finish(&a.id, Ok(out)).unwrap();
        assert!(done.output.unwrap().ends_with("(output cut at 64 KB)"));
    }

    #[test]
    fn the_password_never_reaches_the_ui() {
        let s = ApprovalStore::default();
        let a = s.create(
            "reboot",
            "root@h:22",
            Target::Host {
                host: "h".into(),
                port: 22,
                user: "root".into(),
                password: Some("hunter2".into()),
                host_id: None,
            },
            "mcp",
            vec![],
        );
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("hunter2"), "{json}");
    }
}
