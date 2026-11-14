use super::{matches_counter, signature, Counter, Cursor, Tokens};

pub(super) enum CounterState {
    Replay(String),
    Exact(usize),
    Pending,
}

impl Cursor {
    pub(super) fn counter_state(
        &self,
        key: &str,
        counter: &Counter,
    ) -> Result<CounterState, String> {
        if let Some((identity, previous)) = self.matched_counters.get(key) {
            if previous != &signature(&counter.usage, &counter.cumulative) {
                return Err("Conflicting token notification for a known physical request".into());
            }
            return Ok(CounterState::Replay(identity.clone()));
        }
        if let Some(position) = self
            .pending_exact
            .iter()
            .position(|exact| matches_counter(exact, counter))
        {
            return Ok(CounterState::Exact(position));
        }
        Ok(CounterState::Pending)
    }
}

pub(super) enum DeltaState {
    Unchanged,
    Measured(Tokens),
    Partial(Tokens),
}

impl DeltaState {
    pub fn between(previous: Option<&Tokens>, current: &Tokens, last: &Tokens) -> Self {
        let delta = previous
            .map(|previous| current.difference(previous))
            .unwrap_or_else(|| Some(current.clone()));
        if delta
            .as_ref()
            .is_some_and(|tokens| tokens.input == 0 && tokens.output == 0)
        {
            return Self::Unchanged;
        }
        let partial = delta.as_ref().is_none_or(|tokens| tokens != last);
        let tokens = delta.unwrap_or_else(|| last.clone());
        if partial {
            return Self::Partial(tokens);
        }
        Self::Measured(tokens)
    }

    pub fn measurement(self) -> Option<(Tokens, bool)> {
        match self {
            Self::Unchanged => None,
            Self::Measured(tokens) => Some((tokens, false)),
            Self::Partial(tokens) => Some((tokens, true)),
        }
    }
}
