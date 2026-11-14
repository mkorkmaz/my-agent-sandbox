use super::{http, Provider};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{net::TcpListener, sync::atomic::AtomicBool};

pub(crate) const CLAUDE_CLIENT: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub(crate) const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub(crate) const CLAUDE_SCOPES: &str = "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload user:plugins";
const CHATGPT_CLIENT: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

// Neither challenge nor token material implements Debug or Serialize.
pub(super) enum Challenge {
    Chatgpt {
        device: String,
        code: String,
        interval: u64,
    },
    Claude {
        verifier: String,
        state: String,
        listener: TcpListener,
    },
}

pub(super) fn random() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err("Could not prepare a private sign-in request.".into());
    }
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(super) fn prepare(provider: Provider, cancellation: &AtomicBool) -> Result<Challenge, String> {
    if provider == Provider::Claude {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| "Could not open the local sign-in callback. Retry sign-in.")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "Could not prepare the local sign-in callback.")?;
        return Ok(Challenge::Claude {
            verifier: random()?,
            state: random()?,
            listener,
        });
    }
    let (status, value) = http::request(
        "https://auth.openai.com/api/accounts/deviceauth/usercode",
        Some(json!({"client_id":CHATGPT_CLIENT})),
        None,
        cancellation,
    )?;
    if status != 200 {
        return Err("ChatGPT could not start device sign-in. Retry sign-in.".into());
    }
    device_challenge(&value)
}

pub(super) fn device_challenge(value: &Value) -> Result<Challenge, String> {
    let device = field(value, "device_auth_id")?;
    let code = value["user_code"]
        .as_str()
        .or_else(|| value["usercode"].as_str())
        .ok_or("ChatGPT did not return a device code.")?;
    if !(4..=32).contains(&code.len())
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("ChatGPT returned an invalid device code.".into());
    }
    let interval = match &value["interval"] {
        Value::String(value) => value.parse::<u64>().ok(),
        Value::Number(value) => value.as_u64(),
        Value::Null => Some(5),
        _ => None,
    }
    .filter(|interval| (1..=60).contains(interval))
    .ok_or("ChatGPT returned an invalid polling interval.")?;
    Ok(Challenge::Chatgpt {
        device: device.into(),
        code: code.into(),
        interval,
    })
}

pub(super) fn address(challenge: &Challenge) -> Result<String, String> {
    let Challenge::Claude {
        verifier,
        state,
        listener,
    } = challenge
    else {
        return Ok("https://auth.openai.com/codex/device".into());
    };
    let port = listener
        .local_addr()
        .map_err(|_| "The sign-in callback is unavailable.")?
        .port();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = tauri::Url::parse("https://claude.com/cai/oauth/authorize").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("code", "true"),
        ("client_id", CLAUDE_CLIENT),
        ("response_type", "code"),
        ("redirect_uri", &format!("http://localhost:{port}/callback")),
        ("scope", CLAUDE_SCOPES),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ]);
    Ok(url.into())
}

pub(crate) fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|value| http::valid_token(value))
        .ok_or_else(|| {
            "The provider returned an incomplete sign-in response. Retry sign-in.".into()
        })
}

pub(super) fn exchange_chatgpt(value: Value, cancellation: &AtomicBool) -> Result<Value, String> {
    let fields = [
        ("grant_type", "authorization_code"),
        ("client_id", CHATGPT_CLIENT),
        ("code", field(&value, "authorization_code")?),
        ("code_verifier", field(&value, "code_verifier")?),
        (
            "redirect_uri",
            "https://auth.openai.com/deviceauth/callback",
        ),
    ];
    let (status, value) =
        http::form_request("https://auth.openai.com/oauth/token", &fields, cancellation)?;
    if status != 200 {
        return Err("The provider did not accept sign-in. Start sign-in again in Settings.".into());
    }
    Ok(value)
}

pub(super) fn exchange_claude(
    code: &str,
    verifier: &str,
    state: &str,
    port: u16,
    cancellation: &AtomicBool,
) -> Result<Value, String> {
    exchange(
        CLAUDE_TOKEN_URL,
        json!({"grant_type":"authorization_code", "client_id":CLAUDE_CLIENT,
        "code":code,"code_verifier":verifier,"state":state,"redirect_uri":format!("http://localhost:{port}/callback")}),
        cancellation,
    )
}

fn exchange(url: &str, body: Value, cancellation: &AtomicBool) -> Result<Value, String> {
    let (status, value) = http::request(url, Some(body), None, cancellation)?;
    if status != 200 {
        return Err("The provider did not accept sign-in. Start sign-in again in Settings.".into());
    }
    Ok(value)
}
