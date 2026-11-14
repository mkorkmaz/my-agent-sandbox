use crate::platform::command;
use serde_json::Value;
use std::{path::Path, sync::atomic::AtomicBool, time::Duration};

#[derive(Debug)]
pub(crate) enum Failure {
    Temporary,
    Uncertain,
}

impl From<Failure> for String {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Temporary => {
                "Could not contact the provider. Check your connection and retry."
            }
            Failure::Uncertain => {
                "The provider request did not return a usable response. Retry sign-in in Settings."
            }
        }
        .into()
    }
}

pub(crate) fn request(
    url: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    cancellation: &AtomicBool,
) -> Result<(u16, Value), Failure> {
    let body = body
        .map(|value| serde_json::to_vec(&value))
        .transpose()
        .map_err(|_| Failure::Temporary)?;
    send(url, body, "application/json", bearer, cancellation)
}

pub(super) fn form_request(
    url: &str,
    fields: &[(&str, &str)],
    cancellation: &AtomicBool,
) -> Result<(u16, Value), Failure> {
    let body = form_body(url, fields)?;
    send(
        url,
        Some(body),
        "application/x-www-form-urlencoded",
        None,
        cancellation,
    )
}

pub(super) fn form_body(url: &str, fields: &[(&str, &str)]) -> Result<Vec<u8>, Failure> {
    let mut address = tauri::Url::parse(url).map_err(|_| Failure::Temporary)?;
    address
        .query_pairs_mut()
        .clear()
        .extend_pairs(fields.iter().copied());
    Ok(address.query().unwrap_or_default().as_bytes().to_vec())
}

fn send(
    url: &str,
    body: Option<Vec<u8>>,
    content_type: &str,
    bearer: Option<&str>,
    cancellation: &AtomicBool,
) -> Result<(u16, Value), Failure> {
    let mut arguments: Vec<String> = [
        "-q",
        "--silent",
        "--show-error",
        "--user-agent",
        concat!("Tugboat/", env!("CARGO_PKG_VERSION")),
        "--proto",
        "=https",
        "--connect-timeout",
        "8",
        "--max-time",
        "20",
        "--max-filesize",
        "131072",
        "--header",
        &format!("Content-Type: {content_type}"),
        "--write-out",
        "\n%{http_code}\n%{size_upload}",
        "--url",
        url,
    ]
    .map(str::to_owned)
    .into();
    let input = if let Some(value) = body {
        arguments.extend(["--data-binary".into(), "@-".into()]);
        Some(value)
    } else if let Some(token) = bearer {
        // Keep bearer tokens out of process arguments and diagnostic output.
        if !valid_token(token) {
            return Err(Failure::Temporary);
        }
        arguments.extend(["--header".into(), "@-".into()]);
        Some(format!("Authorization: Bearer {token}\n").into_bytes())
    } else {
        None
    };
    let output = command::run_with_input(
        Path::new("/usr/bin/curl"),
        &arguments,
        None,
        &command::environment(),
        Duration::from_secs(22),
        cancellation,
        input,
    )
    .map_err(|_| Failure::Temporary)?;
    response(output)
}

pub(super) fn response(output: command::CommandOutput) -> Result<(u16, Value), Failure> {
    if output.cancelled || output.output_exceeded {
        return Err(Failure::Uncertain);
    }
    let (body, uploaded) = output.stdout.rsplit_once('\n').ok_or(Failure::Uncertain)?;
    let uploaded: f64 = uploaded.parse().map_err(|_| Failure::Uncertain)?;
    if output.timed_out || output.exit_code != Some(0) {
        if uploaded == 0.0 || matches!(output.exit_code, Some(5 | 6 | 7 | 35 | 60 | 77)) {
            return Err(Failure::Temporary);
        }
        return Err(Failure::Uncertain);
    }
    let (body, status) = body.rsplit_once('\n').ok_or(Failure::Uncertain)?;
    let status: u16 = status.parse().map_err(|_| Failure::Uncertain)?;
    if !(100..600).contains(&status) {
        return Err(Failure::Uncertain);
    }
    let value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(_) if !(200..300).contains(&status) || body.is_empty() => Value::Null,
        Err(_) => return Err(Failure::Uncertain),
    };
    Ok((status, value))
}

pub(crate) fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32_768
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte))
}
