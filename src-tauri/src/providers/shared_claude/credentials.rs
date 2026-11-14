use crate::providers::provider_login::{http::valid_token, protocol::CLAUDE_CLIENT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const INVALID: &str =
    "The shared Claude sign-in is incomplete or unsupported. Sign in again in Settings.";

// Private only. Never add Debug or include this value in an application event.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Credentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub scopes: Vec<String>,
    pub account_id: String,
    pub organization_id: String,
    pub email: Option<String>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
}

impl Credentials {
    pub fn from_response(response: Value, profile: Value) -> Result<Self, String> {
        let expires = response["expires_in"]
            .as_i64()
            .filter(|value| (1..=2_592_000).contains(value))
            .ok_or(INVALID)?;
        if response.get("token_type").is_some_and(|value| {
            value
                .as_str()
                .is_none_or(|value| !value.eq_ignore_ascii_case("bearer"))
        }) {
            return Err(INVALID.into());
        }
        let credential = Self {
            access_token: response["access_token"].as_str().ok_or(INVALID)?.into(),
            refresh_token: response["refresh_token"].as_str().ok_or(INVALID)?.into(),
            expires_at: chrono::Utc::now()
                .timestamp_millis()
                .checked_add(expires * 1000)
                .ok_or(INVALID)?,
            scopes: response["scope"]
                .as_str()
                .ok_or(INVALID)?
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            account_id: profile["account"]["uuid"].as_str().ok_or(INVALID)?.into(),
            organization_id: profile["organization"]["uuid"]
                .as_str()
                .ok_or(INVALID)?
                .into(),
            email: profile["account"]["email"]
                .as_str()
                .or_else(|| profile["account"]["email_address"].as_str())
                .map(str::to_owned),
            subscription_type: match profile["organization"]["organization_type"].as_str() {
                Some("claude_pro") => Some("pro".into()),
                Some("claude_max") => Some("max".into()),
                Some("claude_team") => Some("team".into()),
                Some("claude_enterprise") => Some("enterprise".into()),
                _ => None,
            },
            rate_limit_tier: profile["organization"]["rate_limit_tier"]
                .as_str()
                .map(str::to_owned),
        };
        credential.validate()?;
        Ok(credential)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !valid_token(&self.access_token)
            || !valid_token(&self.refresh_token)
            || self.expires_at <= 0
            || self.scopes.is_empty()
            || self.scopes.len() > 64
            || !self.scopes.iter().any(|scope| scope == "user:inference")
            || self.scopes.iter().any(|scope| {
                scope.len() > 128
                    || !scope
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b":_-".contains(&byte))
            })
            || [&self.account_id, &self.organization_id]
                .iter()
                .any(|value| value.is_empty() || value.len() > 128 || !valid_token(value))
            || self
                .email
                .as_ref()
                .is_some_and(|value| value.len() > 254 || value.chars().any(char::is_control))
            || [&self.subscription_type, &self.rate_limit_tier]
                .iter()
                .any(|value| {
                    value.as_ref().is_some_and(|value| {
                        value.len() > 128 || value.chars().any(char::is_control)
                    })
                })
        {
            return Err(INVALID.into());
        }
        Ok(())
    }

    pub fn renew(&self, response: Value) -> Result<Self, String> {
        let expires = response["expires_in"]
            .as_i64()
            .filter(|value| (1..=2_592_000).contains(value))
            .ok_or(INVALID)?;
        let mut credential = self.clone();
        credential.access_token = response["access_token"].as_str().ok_or(INVALID)?.into();
        credential.expires_at = chrono::Utc::now()
            .timestamp_millis()
            .checked_add(expires * 1000)
            .ok_or(INVALID)?;
        if let Some(token) = response.get("refresh_token") {
            credential.refresh_token = token.as_str().ok_or(INVALID)?.into();
        }
        if let Some(scopes) = response.get("scope") {
            credential.scopes = scopes
                .as_str()
                .ok_or(INVALID)?
                .split_whitespace()
                .map(str::to_owned)
                .collect();
        }
        if response["account"]["uuid"]
            .as_str()
            .is_some_and(|id| id != self.account_id)
            || response["organization"]["uuid"]
                .as_str()
                .is_some_and(|id| id != self.organization_id)
        {
            return Err(INVALID.into());
        }
        credential.validate()?;
        Ok(credential)
    }

    pub fn guest(&self) -> Value {
        // An empty string means a revoked login in Claude Code 2.1.318. Null means host-owned renewal.
        json!({"claudeAiOauth":{"accessToken":self.access_token,"refreshToken":null,"expiresAt":self.expires_at,
            "scopes":self.scopes,"subscriptionType":self.subscription_type,"rateLimitTier":self.rate_limit_tier,
            "clientId":CLAUDE_CLIENT}})
    }
}
