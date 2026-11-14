use super::Provider;
use crate::storage::data_location::RecordsDirectory;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Clone)]
pub(super) struct Attempt {
    pub id: String,
    pub records: RecordsDirectory,
    pub cancellation: Arc<AtomicBool>,
}

impl Attempt {
    pub fn new(records: RecordsDirectory) -> Result<Self, String> {
        Ok(Self {
            id: super::protocol::random()?,
            records,
            cancellation: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[derive(Default)]
pub(super) enum Flow {
    #[default]
    Idle,
    Preparing(Attempt),
    Waiting {
        attempt: Attempt,
        address: String,
        code: Option<String>,
    },
    Exchanging(Attempt),
    Saving(Attempt),
    Failed(String),
}

impl Flow {
    pub fn attempt(&self) -> Option<&Attempt> {
        match self {
            Self::Preparing(attempt)
            | Self::Exchanging(attempt)
            | Self::Saving(attempt)
            | Self::Waiting { attempt, .. } => Some(attempt),
            _ => None,
        }
    }
    pub fn owns(&self, attempt: &Attempt) -> bool {
        self.attempt()
            .is_some_and(|current| current.id == attempt.id)
            && !attempt.cancellation.load(Ordering::SeqCst)
    }
    pub fn cancel(&mut self) -> Result<(), String> {
        if matches!(self, Self::Saving(_)) {
            return Err("The provider connection is being saved. Wait for it to finish.".into());
        }
        if let Some(attempt) = self.attempt() {
            attempt.cancellation.store(true, Ordering::SeqCst);
        }
        *self = Self::Idle;
        Ok(())
    }
    pub fn phase(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Preparing(_) => "preparing",
            Self::Waiting { .. } => "waiting",
            Self::Exchanging(_) => "exchanging",
            Self::Saving(_) => "saving",
            Self::Failed(_) => "failed",
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub provider: Provider,
    pub revision: u64,
    pub phase: &'static str,
    pub authorization_url: Option<String>,
    pub user_code: Option<String>,
    pub message: String,
}

#[derive(Default)]
pub(super) struct Registry {
    pub flows: [Flow; 2],
    pub revision: u64,
}

impl Registry {
    pub fn status(&self, provider: Provider) -> Status {
        let flow = &self.flows[provider.index()];
        let (address, code) = match flow {
            Flow::Waiting { address, code, .. } => (Some(address.clone()), code.clone()),
            _ => (None, None),
        };
        let message = match flow {
            Flow::Idle => "",
            Flow::Preparing(_) => "Preparing provider sign-in…",
            Flow::Waiting { code: Some(_), .. } => {
                "Enter the device code in your browser. This code expires in 15 minutes."
            }
            Flow::Waiting { .. } => {
                "Complete provider sign-in in your browser. Tugboat will connect automatically."
            }
            Flow::Exchanging(_) => "Verifying provider sign-in…",
            Flow::Saving(_) => "Saving the shared provider account…",
            Flow::Failed(message) => message,
        };
        Status {
            provider,
            revision: self.revision,
            phase: flow.phase(),
            authorization_url: address,
            user_code: code,
            message: message.into(),
        }
    }
    pub fn update(&mut self, provider: Provider, flow: Flow) -> Result<Status, String> {
        let current = &self.flows[provider.index()];
        let allowed = match (current, &flow) {
            (Flow::Idle | Flow::Failed(_), Flow::Preparing(_)) => true,
            (Flow::Preparing(_), Flow::Waiting { .. })
            | (Flow::Waiting { .. }, Flow::Exchanging(_))
            | (Flow::Exchanging(_), Flow::Saving(_)) => current
                .attempt()
                .zip(flow.attempt())
                .is_some_and(|(left, right)| left.id == right.id),
            (Flow::Saving(_) | Flow::Idle, Flow::Idle) => true,
            (_, Flow::Failed(_)) => current.attempt().is_some(),
            _ => false,
        };
        if !allowed {
            return Err("The provider sign-in state changed. Start sign-in again.".into());
        }
        self.flows[provider.index()] = flow;
        self.revision = self.revision.saturating_add(1);
        Ok(self.status(provider))
    }
}

#[derive(Default)]
pub struct ProviderLogins(pub(super) Mutex<Registry>);
