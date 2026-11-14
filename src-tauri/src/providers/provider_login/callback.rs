use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(super) fn receive(
    listener: &TcpListener,
    state: &str,
    cancellation: &AtomicBool,
) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(900);
    while Instant::now() < deadline {
        if cancellation.load(Ordering::SeqCst) || crate::desktop::lifecycle::is_closing() {
            return Err("Provider sign-in was cancelled.".into());
        }
        match listener.accept() {
            Ok((mut stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                if let Some(result) = read_callback(&mut stream, state) {
                    return result;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(_) => return Err("The local provider callback stopped. Retry sign-in.".into()),
        }
    }
    Err("Provider sign-in expired after 15 minutes. Start sign-in again.".into())
}

fn read_callback(stream: &mut TcpStream, state: &str) -> Option<Result<String, String>> {
    if stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .is_err()
        || stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .is_err()
    {
        return None;
    }
    let mut bytes = Vec::new();
    while bytes.len() < 16_384 {
        let mut buffer = [0u8; 1024];
        let Ok(count) = stream.read(&mut buffer) else {
            return None;
        };
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            break;
        }
    }
    let parsed = parse(&bytes, state);
    let (status, message) = match &parsed {
        Some(Ok(_)) => (
            "200 OK",
            "Authorization received. Return to Tugboat to check the connection.",
        ),
        _ => (
            "400 Bad Request",
            "This sign-in request is invalid. Return to Tugboat and start sign-in again.",
        ),
    };
    let body = format!("<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>Tugboat sign-in</title><p>{message}</p>");
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}", body.len());
    // A browser can close the tab after sending the callback. It must not discard valid authorization.
    let _ = stream.write_all(response.as_bytes());
    parsed
}

pub(super) fn parse(bytes: &[u8], expected_state: &str) -> Option<Result<String, String>> {
    if bytes.len() > 16_384 || !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
        return None;
    }
    let line = std::str::from_utf8(bytes).ok()?.lines().next()?;
    let mut parts = line.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let target = parts.next()?;
    if !target.starts_with("/callback?") || parts.next()? != "HTTP/1.1" || parts.next().is_some() {
        return None;
    }
    let url = tauri::Url::parse(&format!("http://localhost{target}")).ok()?;
    if url.path() != "/callback" {
        return None;
    }
    let fields: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = fields.iter().filter(|(key, _)| key == "state").collect();
    if states.len() != 1 || states[0].1 != expected_state {
        return None;
    }
    if fields.iter().any(|(key, _)| key == "error") {
        return Some(Err(
            "Claude authorization was declined. Start sign-in again when ready.".into(),
        ));
    }
    let codes: Vec<_> = fields.iter().filter(|(key, _)| key == "code").collect();
    if codes.len() != 1 || !super::http::valid_token(&codes[0].1) {
        return None;
    }
    Some(Ok(codes[0].1.to_string()))
}
