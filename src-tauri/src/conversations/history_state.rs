use super::{environment_history, Dashboard};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum PublicationState {
    Initial,
    Changed,
    Unchanged,
}

impl PublicationState {
    pub fn observe(previous: Option<&Dashboard>, current: &Dashboard) -> Self {
        match previous {
            None => Self::Initial,
            Some(previous) if previous == current => Self::Unchanged,
            Some(_) => Self::Changed,
        }
    }

    pub fn permits_publication(self) -> bool {
        self != Self::Unchanged
    }
}

pub(super) enum HistoryObservation {
    Fresh(Dashboard),
    Retained(Dashboard),
    Saved(Dashboard),
    Unavailable,
}

impl HistoryObservation {
    pub fn dashboard(self, warning: Option<String>) -> Dashboard {
        let degraded = !matches!(&self, Self::Fresh(_));
        let mut dashboard = match self {
            Self::Fresh(dashboard) | Self::Retained(dashboard) | Self::Saved(dashboard) => {
                dashboard
            }
            Self::Unavailable => environment_history::empty(),
        };
        if degraded {
            for conversation in &mut dashboard.conversations {
                conversation.available = false;
            }
        }
        if let Some(warning) = warning.filter(|warning| !dashboard.warnings.contains(warning)) {
            dashboard.warnings.push(warning);
        }
        dashboard
    }
}
