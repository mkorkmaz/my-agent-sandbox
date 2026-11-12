use super::credentials::Credentials;
use crate::command;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub enum Failure {
    SignIn,
    Uncertain,
    Temporary(String),
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Self::SignIn => {
                "The shared ChatGPT session is no longer valid. Sign in again in Settings.".into()
            }
            Self::Temporary(message) => message.clone(),
            Self::Uncertain => "ChatGPT renewal did not return usable credentials. Sign in again in Settings before retrying.".into(),
        }
    }
    pub fn requires_sign_in(&self) -> bool {
        matches!(self, Self::SignIn | Self::Uncertain)
    }
}

fn request(url: &str, input: Vec<u8>, content_type: &str) -> Result<(u16, Value), Failure> {
    let arguments: Vec<String> = [
        "-q",
        "--silent",
        "--show-error",
        "--proto",
        "=https",
        "--connect-timeout",
        "8",
        "--max-time",
        "20",
        "--max-filesize",
        "131072",
        "--header",
        content_type,
        "--data-binary",
        "@-",
        "--write-out",
        "\n%{http_code}\n%{size_upload}",
        "--url",
        url,
    ]
    .map(str::to_owned)
    .into();
    let output = command::run_with_input(
        Path::new("/usr/bin/curl"),
        &arguments,
        None,
        &command::environment(),
        Duration::from_secs(22),
        &AtomicBool::new(false),
        Some(input),
    )
    .map_err(|message| match message.as_str() {
        // The command guard rejected the request before a process could send credentials.
        "The app is closing." => {
            Failure::Temporary("ChatGPT renewal stopped while Tugboat was closing.".into())
        }
        _ => Failure::Uncertain,
    })?;
    response(output)
}

pub fn response(output: command::CommandOutput) -> Result<(u16, Value), Failure> {
    if output.cancelled || output.timed_out || output.output_exceeded {
        return Err(Failure::Uncertain);
    }
    let (body, uploaded) = output.stdout.rsplit_once('\n').ok_or(Failure::Uncertain)?;
    let uploaded: f64 = uploaded.parse().map_err(|_| Failure::Uncertain)?;
    if output.exit_code != Some(0) {
        if uploaded == 0.0 || matches!(output.exit_code, Some(5 | 6 | 7 | 35 | 60 | 77)) {
            return Err(Failure::Temporary(
                "Could not contact ChatGPT. Check your connection and retry.".into(),
            ));
        }
        return Err(Failure::Uncertain);
    }
    let (body, status) = body.rsplit_once('\n').ok_or(Failure::Uncertain)?;
    let status = status.parse().map_err(|_| Failure::Uncertain)?;
    let value = serde_json::from_str(body).unwrap_or(Value::Null);
    Ok((status, value))
}

pub fn refresh(credentials: &Credentials) -> Result<Credentials, Failure> {
    let body = json!({"grant_type":"refresh_token", "client_id":credentials.client().map_err(|_| Failure::SignIn)?,
        "refresh_token":credentials.token("refresh_token").map_err(|_| Failure::SignIn)?});
    let input = serde_json::to_vec(&body).map_err(|_| Failure::SignIn)?;
    let (status, value) = request(
        "https://auth.openai.com/oauth/token",
        input,
        "Content-Type: application/json",
    )?;
    classify_refresh(credentials, status, value)
}

pub fn classify_refresh(
    credentials: &Credentials,
    status: u16,
    value: Value,
) -> Result<Credentials, Failure> {
    if (200..300).contains(&status) {
        // The authority may have rotated the refresh token, so do not retry a malformed success.
        return credentials.renew(value).map_err(|_| Failure::Uncertain);
    }
    let code = value["error"]["code"]
        .as_str()
        .or_else(|| value["error"].as_str());
    if status == 401
        || matches!(
            code,
            Some(
                "invalid_grant"
                    | "refresh_token_expired"
                    | "refresh_token_reused"
                    | "refresh_token_invalidated"
            )
        )
    {
        return Err(Failure::SignIn);
    }
    Err(Failure::Temporary(
        "ChatGPT sign-in could not be renewed. Try again shortly.".into(),
    ))
}

pub fn revoke(credentials: &Credentials) -> bool {
    let Ok(client) = credentials.client() else {
        return false;
    };
    let Ok(token) = credentials.token("refresh_token") else {
        return false;
    };
    let Ok(body) = serde_json::to_vec(
        &json!({"client_id":client,"token":token,"token_type_hint":"refresh_token"}),
    ) else {
        return false;
    };
    request(
        "https://auth.openai.com/oauth/revoke",
        body,
        "Content-Type: application/json",
    )
    .is_ok_and(|(status, _)| (200..300).contains(&status))
}
