use super::Removal;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemovalStep {
    SignOut,
    Environment,
    Binding,
    Policy,
    Record,
}

impl Removal {
    pub fn validate(&self) -> Result<(), String> {
        if (self.policy_handled && !self.binding_detached)
            || (self.binding_detached && !self.environment_removed)
            || (self.logged_out && !self.logout)
            || (self.logout && !self.logged_out && self.environment_removed)
        {
            return Err("The saved removal checkpoints are out of order. Review the removal record.".into());
        }
        Ok(())
    }

    pub fn next_step(&self) -> RemovalStep {
        if self.logout && !self.logged_out {
            return RemovalStep::SignOut;
        }
        if !self.environment_removed {
            return RemovalStep::Environment;
        }
        if !self.binding_detached {
            return RemovalStep::Binding;
        }
        if !self.policy_handled {
            return RemovalStep::Policy;
        }
        RemovalStep::Record
    }

    pub fn needs_stop(&self, running: bool) -> bool {
        self.next_step() == RemovalStep::Environment && (!self.stopped || running)
    }

    pub fn complete(&mut self, step: RemovalStep) -> Result<(), String> {
        self.validate()?;
        if self.next_step() != step || step == RemovalStep::Record {
            return Err("The removal checkpoint changed. Refresh the removal preview.".into());
        }
        match step {
            RemovalStep::SignOut => self.logged_out = true,
            RemovalStep::Environment => self.environment_removed = true,
            RemovalStep::Binding => self.binding_detached = true,
            RemovalStep::Policy => self.policy_handled = true,
            RemovalStep::Record => unreachable!(),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_requires_ordered_checkpoints_and_rechecks_a_restarted_guest() {
        let mut removal = Removal {
            reference: "codex@project".into(),
            workspace: "/project".into(),
            logout: true,
            had_environment: true,
            logged_out: false,
            stopped: false,
            environment_removed: false,
            binding_detached: false,
            policy_handled: false,
            preserved_policy: false,
        };
        assert!(removal.complete(RemovalStep::Policy).is_err());
        removal.complete(RemovalStep::SignOut).unwrap();
        assert!(removal.needs_stop(false));
        removal.stopped = true;
        assert!(!removal.needs_stop(false));
        assert!(removal.needs_stop(true));
        for step in [
            RemovalStep::Environment,
            RemovalStep::Binding,
            RemovalStep::Policy,
        ] {
            assert_eq!(removal.next_step(), step);
            removal.complete(step).unwrap();
        }
        assert_eq!(removal.next_step(), RemovalStep::Record);
        assert!(removal.complete(RemovalStep::Environment).is_err());
    }
}
