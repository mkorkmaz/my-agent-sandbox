use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const INVALID: &str =
    "The saved ChatGPT sign-in is unsupported or incomplete. Sign in with ChatGPT again.";
const LEGACY_CLIENT: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

// Deliberately omit Debug: error output must never contain credential material.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Credentials(Value);

fn claims(token: &str) -> Result<Value, String> {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return Err(INVALID.into());
    }
    let bytes = URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| INVALID)?;
    serde_json::from_slice(&bytes).map_err(|_| INVALID.into())
}

impl Credentials {
    pub fn parse(value: Value, renewable: bool) -> Result<Self, String> {
        if (!value["auth_mode"].is_null() && value["auth_mode"] != "chatgpt")
            || !value["OPENAI_API_KEY"].is_null()
            || !value["agent_identity"].is_null()
            || !value["personal_access_token"].is_null()
        {
            return Err(INVALID.into());
        }
        let credential = Self(value);
        for key in ["id_token", "access_token", "account_id"] {
            let token = credential.token(key)?;
            if token.is_empty() || token.len() > 32_768 || token.chars().any(char::is_control) {
                return Err(INVALID.into());
            }
        }
        let identity = claims(credential.token("id_token")?)?;
        let account = identity["https://api.openai.com/auth"]["chatgpt_account_id"]
            .as_str()
            .ok_or(INVALID)?;
        let access = claims(credential.token("access_token")?)?;
        let selected = credential.account()?;
        let access_account = access["https://api.openai.com/auth"]["chatgpt_account_id"].as_str();
        // Codex retains the original ID token after a workspace exchange.
        let selected_workspace = access_account == Some(selected)
            && identity["https://api.openai.com/auth"]["chatgpt_user_id"]
                .as_str()
                .is_some()
            && identity["https://api.openai.com/auth"]["chatgpt_user_id"]
                == access["https://api.openai.com/auth"]["chatgpt_user_id"];
        if (account != selected && !selected_workspace)
            || credential.expires_at()? <= 0
            || access_account.is_some_and(|value| value != selected)
        {
            return Err(INVALID.into());
        }
        credential.client()?;
        let refresh = credential.token("refresh_token")?;
        if (renewable && refresh.is_empty()) || refresh.len() > 32_768 {
            return Err(INVALID.into());
        }
        Ok(credential)
    }

    pub fn token(&self, name: &str) -> Result<&str, String> {
        self.0["tokens"][name]
            .as_str()
            .ok_or_else(|| INVALID.into())
    }

    pub fn account(&self) -> Result<&str, String> {
        self.token("account_id")
    }

    pub fn selection_key(&self) -> Result<(String, String), String> {
        let identity = claims(self.token("id_token")?)?;
        let user = identity["sub"]
            .as_str()
            .or_else(|| identity["https://api.openai.com/auth"]["chatgpt_user_id"].as_str())
            .unwrap_or(self.token("id_token")?);
        Ok((self.account()?.into(), user.into()))
    }

    pub fn client(&self) -> Result<&str, String> {
        let provenance = &self.0["tokens"]["oauth_provenance"];
        if provenance.is_null() {
            return Ok(LEGACY_CLIENT);
        }
        if provenance["issuer"] != "https://auth.openai.com" {
            return Err(INVALID.into());
        }
        let client = provenance["client_id"].as_str().ok_or(INVALID)?;
        if client.is_empty() || client.len() > 256 || client.chars().any(char::is_control) {
            return Err(INVALID.into());
        }
        Ok(client)
    }

    pub fn expires_at(&self) -> Result<i64, String> {
        claims(self.token("access_token")?)?["exp"]
            .as_i64()
            .ok_or_else(|| INVALID.into())
    }

    pub fn email(&self) -> Option<String> {
        let identity = claims(self.token("id_token").ok()?).ok()?;
        identity["email"]
            .as_str()
            .or_else(|| identity["https://api.openai.com/profile"]["email"].as_str())
            .filter(|email| email.len() <= 254 && !email.chars().any(char::is_control))
            .map(str::to_owned)
    }

    pub fn renew(&self, response: Value) -> Result<Self, String> {
        let mut value = self.0.clone();
        for name in ["id_token", "access_token", "refresh_token"] {
            if let Some(token) = response.get(name) {
                if token.as_str().is_none_or(str::is_empty) {
                    return Err(INVALID.into());
                }
                value["tokens"][name] = token.clone();
            }
        }
        if response["access_token"].as_str().is_none() {
            return Err(INVALID.into());
        }
        value["last_refresh"] = Utc::now().to_rfc3339().into();
        let renewed = Self::parse(value, true)?;
        let previous = claims(self.token("id_token")?)?;
        let current = claims(renewed.token("id_token")?)?;
        if renewed.account()? != self.account()?
            || (!previous["sub"].is_null() && previous["sub"] != current["sub"])
            || (!previous["https://api.openai.com/auth"]["chatgpt_user_id"].is_null()
                && previous["https://api.openai.com/auth"]["chatgpt_user_id"]
                    != current["https://api.openai.com/auth"]["chatgpt_user_id"])
        {
            return Err(INVALID.into());
        }
        Ok(renewed)
    }

    pub fn guest(&self) -> Value {
        let mut value = serde_json::json!({"auth_mode":"chatgpt", "OPENAI_API_KEY":null,
            "tokens":{"id_token":self.0["tokens"]["id_token"],
                "access_token":self.0["tokens"]["access_token"],
                "account_id":self.0["tokens"]["account_id"], "refresh_token":""}});
        for key in ["oauth_provenance", "login_owner_id"] {
            if let Some(field) = self.0["tokens"].get(key) {
                value["tokens"][key] = field.clone();
            }
        }
        // A copied last_refresh from an old login must not trigger guest-owned refresh.
        value["last_refresh"] = Utc::now().to_rfc3339().into();
        value
    }

    pub fn validate(&self) -> Result<(), String> {
        Self::parse(self.0.clone(), true).map(|_| ())
    }
}
