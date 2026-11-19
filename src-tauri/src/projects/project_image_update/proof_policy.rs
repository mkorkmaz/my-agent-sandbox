use crate::desktop::settings::Settings;
use serde_json::{json, Value};
use std::ffi::OsStr;

const CODEX_HOSTS: &[&str] = &["auth.openai.com", "chatgpt.com", "api.openai.com"];
const CLAUDE_HOSTS: &[&str] = &["api.anthropic.com", "platform.claude.com", "claude.ai"];

fn provider_policy_enabled() -> bool {
    std::env::var_os("TUGBOAT_IMAGE_TEST_PROVIDER_POLICY").as_deref() == Some(OsStr::new("1"))
}

pub(super) fn configure(settings: &mut Settings) {
    settings.codex.allowed_hosts.clear();
    settings.claude_code.allowed_hosts.clear();
    if provider_policy_enabled() {
        settings.codex.allowed_hosts = CODEX_HOSTS.iter().map(|host| (*host).into()).collect();
        settings.claude_code.allowed_hosts =
            CLAUDE_HOSTS.iter().map(|host| (*host).into()).collect();
    }
}

pub(crate) fn verify(data: &Value, document: &Value, name: &str) -> Result<(), String> {
    if accepts(data, document, name, provider_policy_enabled()) {
        return Ok(());
    }
    Err("The image proof requires its exact isolated network policy.".into())
}

fn no_rules(rules: &Value) -> bool {
    rules.is_null() || rules.as_array().is_some_and(Vec::is_empty)
}

fn matches_egress(egress: &Value, provider_policy: bool) -> bool {
    if egress["default"] != "deny" || !no_rules(&egress["deny"]) {
        return false;
    }
    let allow = &egress["allow"];
    if !provider_policy {
        return no_rules(allow);
    }
    let Some(allow) = allow.as_array() else {
        return false;
    };
    allow.len() == CODEX_HOSTS.len() + CLAUDE_HOSTS.len()
        && CODEX_HOSTS
            .iter()
            .chain(CLAUDE_HOSTS)
            .all(|host| allow.contains(&json!({"host": host})))
}

fn accepts(data: &Value, document: &Value, name: &str, provider_policy: bool) -> bool {
    data["posture"] == "isolated"
        && data["verify"] == json!({"mode": "require", "policy": "brig"})
        && data["policies"] == json!([name])
        && data["credentials"] == json!([])
        && no_rules(&data["routes"])
        && no_rules(&document["spec"]["routes"])
        && matches_egress(&data["egress"], provider_policy)
        && matches_egress(&document["spec"]["egress"], provider_policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_the_selected_deny_by_default_proof_policy() {
        let name = "phase6-test";
        let allow = CODEX_HOSTS
            .iter()
            .chain(CLAUDE_HOSTS)
            .map(|host| json!({"host": host}))
            .collect::<Vec<_>>();
        let egress = json!({"default": "deny", "allow": allow});
        let data = json!({
            "posture": "isolated", "verify": {"mode": "require", "policy": "brig"},
            "policies": [name], "egress": egress, "credentials": []
        });
        let document = json!({"spec": {"egress": egress}});
        assert!(accepts(&data, &document, name, true));
        assert!(!accepts(&data, &document, name, false));

        let mut extra = data.clone();
        extra["egress"]["allow"]
            .as_array_mut()
            .unwrap()
            .push(json!({"host": "*.openai.com"}));
        assert!(!accepts(&extra, &document, name, true));
        let mut missing = data.clone();
        missing["egress"]["allow"].as_array_mut().unwrap().pop();
        assert!(!accepts(&missing, &document, name, true));
        let mut unknown_credentials = data.clone();
        unknown_credentials["credentials"] = Value::Null;
        assert!(!accepts(&unknown_credentials, &document, name, true));
        let mut routed = document.clone();
        routed["spec"]["routes"] = json!([{"id": "other-route"}]);
        assert!(!accepts(&data, &routed, name, true));

        let mut denied = data.clone();
        denied["egress"]["allow"] = json!([]);
        let mut deny_document = document;
        deny_document["spec"]["egress"]["allow"] = Value::Null;
        assert!(accepts(&denied, &deny_document, name, false));
        assert!(!accepts(&denied, &deny_document, name, true));
    }
}
