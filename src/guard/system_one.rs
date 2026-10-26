//! System One — SenClaw's local typed-decision runtime — as a second opinion.
//!
//! The daemon serves it at `POST /api/decision/ask` (proxied onto whichever
//! runtime fills the `decision` slot). A Space App has no bridge action for it,
//! so this calls that route directly with the identity headers the SDK sends.
//!
//! It is only ever allowed to *raise* a command to the admin, never to let one
//! through: measured 2026-10-26 on the local `multilingual` checkpoint over 60
//! labelled server commands, the best wording got 45 right and still read
//! `truncate -s 0 /var/log/syslog` and `cat ~/.ssh/id_rsa` as harmless at 0.95.
//! At confidence >= 0.6 its "change" verdict caught 21 of 30 risky commands and
//! wrongly flagged 3 of 30 harmless ones.

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

/// A cold start loads a ~1.3 GB checkpoint (a second or two); the rest is ms.
const ASK_TIMEOUT: Duration = Duration::from_secs(60);

/// The wording that measured best (45/60); see the module docs. Re-measure
/// before changing it — longer descriptions made other System One uses worse.
pub const INSTRUCTIONS: &str = "What does this shell command do to the server?";
pub const VIEW: &str = "view";
pub const CHANGE: &str = "change";
pub const OPTIONS: &[(&str, &str)] = &[
    (VIEW, "Only looks at status, logs, files or settings"),
    (
        CHANGE,
        "Changes, deletes, restarts, stops, installs something, or reads passwords and private keys",
    ),
];

/// Why a question went unanswered: `Unavailable` is the service (not
/// installed, not running, refusing this app); `Rejected` is this input.
#[derive(Debug, Clone, PartialEq)]
pub enum AskError {
    Unavailable(String),
    Rejected(String),
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AskError::Unavailable(m) | AskError::Rejected(m) => f.write_str(m),
        }
    }
}

/// What System One chose, and how sure it was.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub choice: String,
    /// `max(p)` over the options. A weak signal: it is sometimes wrong at 0.9+.
    pub confidence: f64,
}

fn jstr(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// The wire text for one `choice` question. Built by hand rather than through
/// `serde_json::Value`: the order of `criteria` is the order of the options the
/// model reads, and without `preserve_order` a `Value` sorts them alphabetically.
pub fn build_request(text: &str, instructions: &str, options: &[(&str, &str)]) -> String {
    let criteria = options
        .iter()
        .map(|(key, description)| format!("{}:{}", jstr(key), jstr(description)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"state\":{},\"questions\":{{\"answer\":{{\"type\":\"choice\",\"instructions\":{},\"criteria\":{{{criteria}}}}}}}}}",
        jstr(text),
        jstr(instructions),
    )
}

pub fn parse_response(text: &str) -> Result<Verdict, AskError> {
    let rejected = |m: String| AskError::Rejected(m);
    let v: Value = serde_json::from_str(text)
        .map_err(|e| rejected(format!("System One answered with something that is not JSON: {e}")))?;
    let a = v
        .pointer("/answers/answer")
        .ok_or_else(|| rejected("System One did not answer the question".to_string()))?;
    let choice = a
        .get("choice")
        .and_then(Value::as_str)
        .ok_or_else(|| rejected("System One picked no option".to_string()))?
        .to_string();
    let confidence = a
        .get("answer_confidence")
        .or_else(|| a.get("confidence"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Ok(Verdict { choice, confidence })
}

pub fn base_url() -> String {
    std::env::var("SENCLAW_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:18788".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn daemon_headers() -> reqwest::header::HeaderMap {
    use app_space_sdk::{
        HEADER_API_VERSION, HEADER_APP_TOKEN, api_version_from_env, app_token_from_env,
    };
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    let mut h = HeaderMap::new();
    if let Some(t) = app_token_from_env() {
        if let Ok(mut v) = HeaderValue::from_str(&t) {
            v.set_sensitive(true);
            h.insert(HeaderName::from_static(HEADER_APP_TOKEN), v);
        }
    }
    if let Ok(v) = HeaderValue::from_str(&api_version_from_env().to_string()) {
        h.insert(HeaderName::from_static(HEADER_API_VERSION), v);
    }
    h
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(ASK_TIMEOUT)
            .default_headers(daemon_headers())
            .build()
            .unwrap_or_default()
    })
}

/// A refusal of the daemon, in words for the person: the proxy answers `503`
/// with a machine-readable `code` when no decision runtime is installed/selected.
fn explain_failure(status: u16, body: &str) -> AskError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let code = v.get("code").and_then(Value::as_str).unwrap_or("");
    let detail = v
        .get("error")
        .or_else(|| v.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let unavailable = AskError::Unavailable;
    match code {
        "runtime_not_installed" => unavailable(
            "System One is not installed (install the sen-sysone runtime in SenClaw Settings → Runtimes)".into(),
        ),
        "runtime_not_selected" => {
            unavailable("No runtime is selected for System One in SenClaw Settings → Runtimes".into())
        }
        "runtime_start_failed" => unavailable(format!("System One could not start: {detail}")),
        _ if status == 503 => unavailable(
            format!("System One is not ready: {detail}")
                .trim_end_matches(": ")
                .to_string(),
        ),
        _ if status == 426 => unavailable("SenClaw is too old for this app".into()),
        _ if status == 401 || status == 403 => {
            unavailable("SenClaw refused this app's call to System One (token)".into())
        }
        _ => {
            let d: String = if detail.is_empty() {
                body.chars().take(160).collect()
            } else {
                detail.to_string()
            };
            let msg = format!("System One error {status}: {d}");
            // A server fault is the service's; any other 4xx is this input's.
            if status >= 500 { unavailable(msg) } else { AskError::Rejected(msg) }
        }
    }
}

/// Ask System One whether `command` only looks or changes something.
pub async fn ask(command: &str) -> Result<Verdict, AskError> {
    let url = format!("{}/api/decision/ask", base_url());
    let resp = client()
        .post(&url)
        .header("content-type", "application/json")
        .body(build_request(command, INSTRUCTIONS, OPTIONS))
        .send()
        .await
        .map_err(|e| AskError::Unavailable(format!("Could not reach SenClaw ({url}): {e}")))?;
    let status = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| AskError::Unavailable(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(explain_failure(status, &body));
    }
    let verdict = parse_response(&body)?;
    if !OPTIONS.iter().any(|(key, _)| *key == verdict.choice) {
        return Err(AskError::Rejected(format!(
            "System One chose '{}', which was not one of the options",
            verdict.choice
        )));
    }
    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_keeps_option_order_and_escapes_text() {
        let req = build_request("echo \"x\" | grep 'y'\nls", INSTRUCTIONS, OPTIONS);
        assert!(req.find("\"view\"").unwrap() < req.find("\"change\"").unwrap());
        let v: Value = serde_json::from_str(&req).expect("valid json");
        assert_eq!(v["state"], "echo \"x\" | grep 'y'\nls");
        assert_eq!(v["questions"]["answer"]["type"], "choice");
    }

    #[test]
    fn parses_choice_and_confidence() {
        let v = parse_response(
            r#"{"answers":{"answer":{"choice":"change","answer_confidence":0.83}},"model":"multilingual"}"#,
        )
        .unwrap();
        assert_eq!(v, Verdict { choice: "change".into(), confidence: 0.83 });
        let v = parse_response(r#"{"answers":{"answer":{"choice":"view","confidence":0.4}}}"#).unwrap();
        assert_eq!(v.confidence, 0.4);
        assert!(parse_response("not json").is_err());
        assert!(parse_response(r#"{"answers":{"answer":{"confidence":0.9}}}"#).is_err());
    }

    #[test]
    fn service_faults_and_input_faults_are_told_apart() {
        assert!(explain_failure(503, r#"{"code":"runtime_not_installed"}"#).to_string().contains("sen-sysone"));
        for status in [401, 403, 426, 500, 502, 503] {
            assert!(matches!(explain_failure(status, "{}"), AskError::Unavailable(_)), "{status}");
        }
        assert!(matches!(explain_failure(422, "{}"), AskError::Rejected(_)));
    }
}
