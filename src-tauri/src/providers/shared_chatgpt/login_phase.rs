use super::store::PendingLogin;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoginPhase {
    Pending,
    Imported,
    Cancelled,
}

impl PendingLogin {
    pub fn phase(&self) -> LoginPhase {
        if self.imported {
            return LoginPhase::Imported;
        }
        if self.cancelled {
            return LoginPhase::Cancelled;
        }
        LoginPhase::Pending
    }

    pub fn import(&mut self) -> Result<(), String> {
        if self.phase() != LoginPhase::Pending {
            return Err("This ChatGPT sign-in is no longer pending.".into());
        }
        self.imported = true;
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), String> {
        if self.phase() == LoginPhase::Imported {
            return Err(
                "The new account is saved. Select Retry connection to finish connecting it.".into(),
            );
        }
        self.cancelled = true;
        Ok(())
    }
}
