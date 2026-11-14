use super::{
    callback, http, protocol,
    state::{Attempt, Flow, Registry},
    Provider,
};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tugboat-provider-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
    fn attempt(&self) -> Attempt {
        Attempt::new(crate::storage::data_location::RecordsDirectory::fixture(
            &self.0,
        ))
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn sign_in_transitions_reject_skipped_stages_and_results_from_other_attempts() {
    let fixture = Fixture::new();
    let first = fixture.attempt();
    let second = fixture.attempt();
    let mut registry = Registry::default();
    assert!(registry
        .update(Provider::Claude, Flow::Saving(first.clone()))
        .is_err());
    registry
        .update(Provider::Claude, Flow::Preparing(first.clone()))
        .unwrap();
    assert!(registry
        .update(Provider::Claude, Flow::Preparing(second.clone()))
        .is_err());
    assert!(registry
        .update(
            Provider::Claude,
            Flow::Waiting {
                attempt: second,
                address: "https://claude.com/".into(),
                code: None
            }
        )
        .is_err());
    registry
        .update(
            Provider::Claude,
            Flow::Waiting {
                attempt: first.clone(),
                address: "https://claude.com/".into(),
                code: None,
            },
        )
        .unwrap();
    registry
        .update(Provider::Claude, Flow::Exchanging(first.clone()))
        .unwrap();
    registry
        .update(Provider::Claude, Flow::Saving(first))
        .unwrap();
    assert!(registry.flows[1].cancel().is_err());
    registry.update(Provider::Claude, Flow::Idle).unwrap();
    assert_eq!(registry.status(Provider::Claude).phase, "idle");
    assert_eq!(registry.status(Provider::Chatgpt).phase, "idle");
}

#[test]
fn cancellation_invalidates_late_results_without_changing_the_other_provider() {
    let fixture = Fixture::new();
    let attempt = fixture.attempt();
    let mut registry = Registry::default();
    registry
        .update(Provider::Chatgpt, Flow::Preparing(attempt.clone()))
        .unwrap();
    registry
        .update(Provider::Claude, Flow::Preparing(fixture.attempt()))
        .unwrap();
    registry.flows[0].cancel().unwrap();
    assert!(attempt.cancellation.load(Ordering::SeqCst));
    assert!(!registry.flows[0].owns(&attempt));
    assert_eq!(registry.status(Provider::Claude).phase, "preparing");
    let json = serde_json::to_string(&registry.status(Provider::Chatgpt)).unwrap();
    assert!(!json.contains(&attempt.id));
    assert!(!json.contains("records"));
}

#[test]
fn callback_requires_one_matching_state_and_one_bounded_code() {
    let request = |query: &str| {
        format!("GET /callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n").into_bytes()
    };
    assert_eq!(
        callback::parse(&request("code=valid-code&state=expected"), "expected"),
        Some(Ok("valid-code".into()))
    );
    for query in [
        "code=valid-code&state=foreign",
        "code=valid-code",
        "code=one&code=two&state=expected",
        "code=valid&state=expected&state=expected",
        "code=bad%0Aheader&state=expected",
    ] {
        assert!(callback::parse(&request(query), "expected").is_none());
    }
    assert!(
        callback::parse(&request("error=access_denied&state=expected"), "expected")
            .unwrap()
            .is_err()
    );
    assert!(callback::parse(
        b"POST /callback?code=valid&state=expected HTTP/1.1\r\n\r\n",
        "expected"
    )
    .is_none());
    assert!(callback::parse(
        b"GET /callback?code=valid&state=expected HTTP/1.1\r\n",
        "expected"
    )
    .is_none());
    assert!(callback::parse(&vec![b'A'; 16_385], "expected").is_none());
}

#[test]
fn device_responses_reject_missing_invalid_and_oversized_fields() {
    let value = json!({"device_auth_id":"device-id","user_code":"ABCD-EFGH","interval":"5"});
    assert!(protocol::device_challenge(&value).is_ok());
    for (key, replacement) in [
        ("device_auth_id", json!(null)),
        ("user_code", json!("bad\ncode")),
        ("user_code", json!("X".repeat(33))),
        ("interval", json!(0)),
        ("interval", json!(61)),
        ("interval", json!(-1)),
    ] {
        let mut invalid = value.clone();
        invalid[key] = replacement;
        assert!(protocol::device_challenge(&invalid).is_err());
    }
}

#[test]
fn authorization_code_form_preserves_tokens_and_redirect_address() {
    let fields = [
        ("code", "code+/="),
        ("code_verifier", "verifier-._~"),
        (
            "redirect_uri",
            "https://auth.openai.com/deviceauth/callback",
        ),
    ];
    let body = http::form_body("https://auth.openai.com/oauth/token", &fields).unwrap();
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains("code=code%2B%2F%3D"));
    assert!(body.contains("redirect_uri=https%3A%2F%2Fauth.openai.com%2Fdeviceauth%2Fcallback"));
    let address = tauri::Url::parse(&format!("https://auth.openai.com/?{body}")).unwrap();
    assert_eq!(
        address.query_pairs().into_owned().collect::<Vec<_>>(),
        fields.map(|(key, value)| (key.to_owned(), value.to_owned()))
    );
}

#[test]
fn transport_failures_distinguish_requests_not_sent_from_uncertain_rotations() {
    let output = |stdout: &str, code| crate::platform::command::CommandOutput {
        stdout: stdout.into(),
        stderr: "synthetic-private-error".into(),
        exit_code: Some(code),
        timed_out: false,
        cancelled: false,
        output_exceeded: false,
    };
    assert!(matches!(
        http::response(output("\n000\n0", 6)),
        Err(http::Failure::Temporary)
    ));
    assert!(matches!(
        http::response(output("\n000\n100", 28)),
        Err(http::Failure::Uncertain)
    ));
    assert!(matches!(
        http::response(output("malformed\n200\n100", 0)),
        Err(http::Failure::Uncertain)
    ));
    assert_eq!(
        http::response(output("<html>unavailable</html>\n503\n100", 0))
            .unwrap()
            .0,
        503
    );
    assert!(!String::from(http::Failure::Uncertain).contains("synthetic-private-error"));
}

#[test]
fn claude_authorization_uses_a_private_loopback_callback_and_pkce_without_project_data() {
    let challenge =
        protocol::prepare(Provider::Claude, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    let address = tauri::Url::parse(&protocol::address(&challenge).unwrap()).unwrap();
    assert_eq!(address.host_str(), Some("claude.com"));
    let fields: std::collections::HashMap<_, _> = address.query_pairs().collect();
    assert_eq!(fields.get("code_challenge_method").unwrap(), "S256");
    assert!(fields
        .get("redirect_uri")
        .unwrap()
        .starts_with("http://localhost:"));
    assert!(!fields.get("scope").unwrap().contains("create_api_key"));
    assert!(!fields.contains_key("project"));
    assert!(!fields.contains_key("code_verifier"));
    assert_eq!(fields.get("state").unwrap().len(), 43);
}
