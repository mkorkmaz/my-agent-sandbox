use super::{credentials::Credentials, store::Session};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AccountState {
    Unconnected,
    Disconnected,
    SigningIn,
    SignedIn,
    NeedsSignIn,
}

impl Session {
    pub(super) fn generation_after(&self, changes: u64) -> Result<u64, String> {
        self.generation.checked_add(changes)
            .ok_or_else(|| "The account record generation is invalid.".into())
    }

    pub(super) fn account_state(&self) -> AccountState {
        if self.pending_login.is_some() {
            return AccountState::SigningIn;
        }
        if self.credentials.is_some() && self.reconnect_required {
            return AccountState::NeedsSignIn;
        }
        if self.credentials.is_some() {
            return AccountState::SignedIn;
        }
        if self.disconnected {
            return AccountState::Disconnected;
        }
        AccountState::Unconnected
    }

    pub fn connect(&mut self, credentials: Credentials) -> Result<(), String> {
        let generation = self.generation_after(1)?;
        self.credentials = Some(credentials);
        self.disconnected = false;
        self.reconnect_required = false;
        self.generation = generation;
        Ok(())
    }

    pub fn require_sign_in(&mut self) -> Result<(), String> {
        if self.credentials.is_none() {
            return Err("Connect the shared account before requesting sign-in renewal.".into());
        }
        let generation = self.generation_after(1)?;
        self.reconnect_required = true;
        self.generation = generation;
        Ok(())
    }

    pub fn disconnect(&mut self) -> Result<(), String> {
        if self.pending_login.is_some() {
            return Err("Complete or cancel the current ChatGPT sign-in first.".into());
        }
        let generation = self.generation_after(1)?;
        self.credentials = None;
        self.disconnected = true;
        self.reconnect_required = false;
        for binding in &mut self.bindings {
            binding.pending = false;
        }
        self.generation = generation;
        Ok(())
    }
}
