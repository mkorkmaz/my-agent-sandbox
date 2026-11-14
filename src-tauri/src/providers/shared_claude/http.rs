use super::credentials::Credentials;
use crate::providers::provider_login::{
    http,
    protocol::{CLAUDE_CLIENT, CLAUDE_TOKEN_URL},
};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

pub(super) enum Failure {
    SignIn,
    Uncertain,
    Temporary,
}
impl Failure {
    pub fn message(&self) -> &'static str {
        match self {
            Self::SignIn | Self::Uncertain => {
                "The shared Claude account needs sign-in. Sign in again in Settings."
            }
            Self::Temporary => {
                "Could not renew the Claude account. Check your connection and retry."
            }
        }
    }
    pub fn needs_sign_in(&self) -> bool {
        !matches!(self, Self::Temporary)
    }
}

pub(super) fn profile(response: &Value, cancellation: &AtomicBool) -> Result<Value, String> {
    let token = super::super::provider_login::protocol::field(response, "access_token")?;
    let (status, value) = http::request(
        "https://api.anthropic.com/api/oauth/profile",
        None,
        Some(token),
        cancellation,
    )?;
    if status != 200 {
        return Err(
            "Claude could not verify this account. Start sign-in again in Settings.".into(),
        );
    }
    Ok(value)
}

pub(super) fn refresh(credentials: &Credentials) -> Result<Credentials, Failure> {
    let (status, value) = http::request(CLAUDE_TOKEN_URL, Some(json!({"grant_type":"refresh_token",
        "refresh_token":credentials.refresh_token,"client_id":CLAUDE_CLIENT,"scope":credentials.scopes.join(" ")})),
        None, &AtomicBool::new(false)).map_err(|failure| match failure { http::Failure::Temporary => Failure::Temporary, http::Failure::Uncertain => Failure::Uncertain })?;
    classify(credentials, status, value)
}

pub(super) fn classify(
    credentials: &Credentials,
    status: u16,
    value: Value,
) -> Result<Credentials, Failure> {
    if status == 200 {
        return credentials.renew(value).map_err(|_| Failure::Uncertain);
    }
    if status == 401
        || value["error"] == "invalid_grant"
        || value["error"]["type"] == "invalid_grant"
    {
        return Err(Failure::SignIn);
    }
    Err(Failure::Temporary)
}

pub(super) fn revoke(credentials: &Credentials) -> bool {
    http::request(
        &format!("{CLAUDE_TOKEN_URL}/revoke"),
        Some(json!({"token":credentials.refresh_token,
        "token_type_hint":"refresh_token","client_id":CLAUDE_CLIENT})),
        None,
        &AtomicBool::new(false),
    )
    .is_ok_and(|(status, _)| (200..300).contains(&status))
}
