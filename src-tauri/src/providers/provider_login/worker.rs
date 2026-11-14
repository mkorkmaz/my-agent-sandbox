use super::{
    callback, http,
    protocol::{self, Challenge},
    state::{Attempt, Flow},
    Provider, ProviderLogins,
};
use serde_json::{json, Value};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

enum Granted {
    Chatgpt(crate::providers::shared_chatgpt::Credentials),
    Claude(crate::providers::shared_claude::Credentials),
}

pub(super) fn run(app: AppHandle, provider: Provider, attempt: Attempt) {
    if let Err(message) = authenticate(&app, provider, &attempt) {
        update(&app, provider, &attempt, Flow::Failed(message));
    }
}

fn update(app: &AppHandle, provider: Provider, attempt: &Attempt, flow: Flow) -> bool {
    let state = app.state::<ProviderLogins>();
    let Ok(mut registry) = state.0.lock() else {
        return false;
    };
    if !registry.flows[provider.index()].owns(attempt) {
        return false;
    }
    let Ok(status) = registry.update(provider, flow) else {
        return false;
    };
    super::announce(app, &status);
    true
}

fn authenticate(app: &AppHandle, provider: Provider, attempt: &Attempt) -> Result<(), String> {
    attempt.records.verify()?;
    match provider {
        Provider::Chatgpt => crate::providers::shared_chatgpt::prepare_provider_login(app)?,
        Provider::Claude => crate::providers::shared_claude::prepare_provider_login(app)?,
    }
    let challenge = protocol::prepare(provider, &attempt.cancellation)?;
    let address = protocol::address(&challenge)?;
    let code = match &challenge {
        Challenge::Chatgpt { code, .. } => Some(code.clone()),
        _ => None,
    };
    if !update(
        app,
        provider,
        attempt,
        Flow::Waiting {
            attempt: attempt.clone(),
            address: address.clone(),
            code,
        },
    ) {
        return Ok(());
    }
    if attempt.cancellation.load(Ordering::SeqCst) {
        return Ok(());
    }
    super::open(&address)?;
    let response = authorize(app, provider, attempt, challenge)?;
    let granted = match provider {
        Provider::Chatgpt => {
            crate::providers::shared_chatgpt::Credentials::from_tokens(response.clone())
                .map(Granted::Chatgpt)
        }
        Provider::Claude => {
            crate::providers::shared_claude::credentials(&response, &attempt.cancellation)
                .map(Granted::Claude)
        }
    };
    let granted = match granted {
        Ok(granted) => granted,
        Err(message) => {
            revoke(provider, &response);
            return Err(message);
        }
    };
    let current = crate::storage::data_location::records_directory(app);
    if current.is_err()
        || current.is_ok_and(|current| current.id() != attempt.records.id())
        || attempt.records.verify().is_err()
    {
        revoke(provider, &response);
        return Err("The records folder changed during sign-in. Start sign-in again.".into());
    }
    if !update(app, provider, attempt, Flow::Saving(attempt.clone())) {
        revoke(provider, &response);
        return Ok(());
    }
    match granted {
        Granted::Chatgpt(credentials) => {
            crate::providers::shared_chatgpt::connect_provider(app, &attempt.records, credentials)?
        }
        Granted::Claude(credentials) => {
            crate::providers::shared_claude::connect_provider(app, &attempt.records, credentials)?
        }
    }
    update(app, provider, attempt, Flow::Idle);
    Ok(())
}

fn authorize(
    app: &AppHandle,
    provider: Provider,
    attempt: &Attempt,
    challenge: Challenge,
) -> Result<Value, String> {
    match challenge {
        Challenge::Chatgpt {
            device,
            code,
            interval,
        } => {
            let response = poll_device(&device, &code, interval, &attempt.cancellation)?;
            if !update(app, provider, attempt, Flow::Exchanging(attempt.clone())) {
                return Err("Provider sign-in was cancelled.".into());
            }
            protocol::exchange_chatgpt(response, &attempt.cancellation)
        }
        Challenge::Claude {
            verifier,
            state,
            listener,
        } => {
            let code = callback::receive(&listener, &state, &attempt.cancellation)?;
            let port = listener
                .local_addr()
                .map_err(|_| "The sign-in callback is unavailable.")?
                .port();
            if !update(app, provider, attempt, Flow::Exchanging(attempt.clone())) {
                return Err("Provider sign-in was cancelled.".into());
            }
            protocol::exchange_claude(&code, &verifier, &state, port, &attempt.cancellation)
        }
    }
}

fn poll_device(
    device: &str,
    code: &str,
    interval: u64,
    cancellation: &AtomicBool,
) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(900);
    while Instant::now() < deadline {
        if cancellation.load(Ordering::SeqCst) || crate::desktop::lifecycle::is_closing() {
            return Err("Provider sign-in was cancelled.".into());
        }
        let (status, value) = http::request(
            "https://auth.openai.com/api/accounts/deviceauth/token",
            Some(json!({"device_auth_id":device,"user_code":code})),
            None,
            cancellation,
        )?;
        if status == 200 {
            return Ok(value);
        }
        if !matches!(status, 403 | 404) {
            return Err("ChatGPT did not accept device sign-in. Start sign-in again.".into());
        }
        let next = (Instant::now() + Duration::from_secs(interval)).min(deadline);
        while Instant::now() < next && !cancellation.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err("ChatGPT sign-in expired after 15 minutes. Start sign-in again.".into())
}

fn revoke(provider: Provider, response: &Value) {
    let Ok(token) = protocol::field(response, "refresh_token") else {
        return;
    };
    let (url, client) = match provider {
        Provider::Chatgpt => (
            "https://auth.openai.com/oauth/revoke",
            "app_EMoamEEZ73f0CkXaXp7hrann",
        ),
        Provider::Claude => (
            "https://platform.claude.com/v1/oauth/token/revoke",
            protocol::CLAUDE_CLIENT,
        ),
    };
    let acknowledged = http::request(
        url,
        Some(json!({"token":token,"token_type_hint":"refresh_token","client_id":client})),
        None,
        &AtomicBool::new(false),
    )
    .is_ok_and(|(status, _)| (200..300).contains(&status));
    if !acknowledged {
        eprintln!("Remote revocation of an unused provider sign-in was not confirmed.");
    }
}
