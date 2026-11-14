pub(super) enum SessionSelection {
    New,
    Adopt(String),
    Choose(Vec<String>),
}

impl SessionSelection {
    pub(super) fn resolve(
        mut candidates: Vec<String>,
        chosen: Option<&str>,
    ) -> Result<Self, String> {
        if let Some(chosen) = chosen {
            let position = candidates
                .iter()
                .position(|candidate| candidate == chosen)
                .ok_or("The selected Brig session does not belong to this project folder.")?;
            return Ok(Self::Adopt(candidates.remove(position)));
        }
        match candidates.len() {
            0 => Ok(Self::New),
            1 => Ok(Self::Adopt(candidates.remove(0))),
            _ => Ok(Self::Choose(candidates)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_current_candidate_can_leave_the_choice_state() {
        let candidates = vec!["codex@one".into(), "codex@two".into()];
        assert!(matches!(
            SessionSelection::resolve(candidates.clone(), None).unwrap(),
            SessionSelection::Choose(_)
        ));
        assert!(
            matches!(SessionSelection::resolve(candidates.clone(), Some("codex@two")).unwrap(), SessionSelection::Adopt(reference) if reference == "codex@two")
        );
        assert!(SessionSelection::resolve(candidates, Some("codex@foreign")).is_err());
        assert!(SessionSelection::resolve(Vec::new(), Some("codex@foreign")).is_err());
        assert!(matches!(
            SessionSelection::resolve(Vec::new(), None).unwrap(),
            SessionSelection::New
        ));
        assert!(matches!(
            SessionSelection::resolve(vec!["codex@one".into()], None).unwrap(),
            SessionSelection::Adopt(_)
        ));
    }
}
