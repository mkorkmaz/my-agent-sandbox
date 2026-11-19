use crate::projects::EnvironmentId;
use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Mutex, OnceLock};

type EnvironmentScope = (String, EnvironmentId);
static ACTIVE: OnceLock<Mutex<HashSet<EnvironmentScope>>> = OnceLock::new();

fn active() -> &'static Mutex<HashSet<EnvironmentScope>> {
    ACTIVE.get_or_init(Default::default)
}

pub(crate) struct EnvironmentOperation(EnvironmentScope);

impl EnvironmentOperation {
    pub fn begin(project_id: &str, environment_id: &EnvironmentId) -> Result<Self, String> {
        let scope = (project_id.to_owned(), environment_id.clone());
        let mut operations = active().lock().map_err(|error| error.to_string())?;
        if !operations.insert(scope.clone()) {
            return Err(
                "This environment operation is already in progress. Wait and retry.".into(),
            );
        }
        Ok(Self(scope))
    }

    pub fn pending(project_id: &str, environment_id: &EnvironmentId) -> bool {
        active()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&(project_id.to_owned(), environment_id.clone()))
    }
}

impl Drop for EnvironmentOperation {
    fn drop(&mut self) {
        active()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.0);
    }
}

type PreparationResult = Result<u64, String>;

struct Starting {
    fingerprint: String,
    waiting: Vec<mpsc::Sender<PreparationResult>>,
}

fn preparations() -> &'static Mutex<HashMap<EnvironmentScope, Starting>> {
    static PREPARATIONS: OnceLock<Mutex<HashMap<EnvironmentScope, Starting>>> = OnceLock::new();
    PREPARATIONS.get_or_init(Default::default)
}

pub(super) enum SharedPreparation {
    Start(PreparationRun),
    Join(mpsc::Receiver<PreparationResult>),
}

impl SharedPreparation {
    pub(super) fn begin(
        project: &str,
        environment: &EnvironmentId,
        fingerprint: &str,
    ) -> Result<Self, String> {
        let scope = (project.to_owned(), environment.clone());
        let mut active = preparations().lock().map_err(|error| error.to_string())?;
        if let Some(starting) = active.get_mut(&scope) {
            if starting.fingerprint != fingerprint {
                return Err("A different project plan is preparing. Review its result before starting this plan.".into());
            }
            let (sender, receiver) = mpsc::channel();
            starting.waiting.push(sender);
            return Ok(Self::Join(receiver));
        }
        let operation = EnvironmentOperation::begin(project, environment)?;
        active.insert(
            scope.clone(),
            Starting {
                fingerprint: fingerprint.into(),
                waiting: Vec::new(),
            },
        );
        Ok(Self::Start(PreparationRun {
            scope,
            operation: Some(operation),
        }))
    }
}

pub(super) struct PreparationRun {
    scope: EnvironmentScope,
    operation: Option<EnvironmentOperation>,
}

impl PreparationRun {
    pub(super) fn finish(mut self, result: PreparationResult) -> PreparationResult {
        self.release(&result);
        result
    }

    fn release(&mut self, result: &PreparationResult) {
        let Some(operation) = self.operation.take() else {
            return;
        };
        let mut active = preparations()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let starting = active.remove(&self.scope);
        drop(operation);
        if let Some(starting) = starting {
            for sender in starting.waiting {
                let _ = sender.send(result.clone());
            }
        }
    }
}

impl Drop for PreparationRun {
    fn drop(&mut self) {
        self.release(&Err(
            "Project preparation was interrupted. Try again.".into()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unpublished_starts_and_stops_share_admission_until_the_operation_returns() {
        let project = "gate-test";
        let id = EnvironmentId::for_project(project);
        let operation = EnvironmentOperation::begin(project, &id).unwrap();
        assert!(EnvironmentOperation::pending(project, &id));
        assert!(EnvironmentOperation::begin(project, &id).is_err());
        let other = EnvironmentId::for_project("gate-other");
        let other_project = EnvironmentOperation::begin("gate-other", &other).unwrap();
        drop(operation);
        assert!(!EnvironmentOperation::pending(project, &id));
        assert!(EnvironmentOperation::pending("gate-other", &other));
        assert!(EnvironmentOperation::begin(project, &id).is_ok());
        drop(other_project);
    }

    #[test]
    fn matching_preparations_join_before_the_terminal_exists() {
        let project = "delayed-shared-preparation";
        let environment = EnvironmentId::for_project(project);
        for result in [Ok(42), Err("Synthetic package failure".into())] {
            let SharedPreparation::Start(owner) =
                SharedPreparation::begin(project, &environment, "checked").unwrap()
            else {
                panic!("The first request must own the preparation");
            };
            let SharedPreparation::Join(waiter) =
                SharedPreparation::begin(project, &environment, "checked").unwrap()
            else {
                panic!("The second request must wait for the original preparation");
            };
            assert_eq!(waiter.try_recv(), Err(mpsc::TryRecvError::Empty));
            assert!(SharedPreparation::begin(project, &environment, "conflicting").is_err());
            assert!(EnvironmentOperation::begin(project, &environment).is_err());
            assert_eq!(owner.finish(result.clone()), result);
            assert_eq!(waiter.recv().unwrap(), result);
            assert!(!EnvironmentOperation::pending(project, &environment));
        }
        let SharedPreparation::Start(owner) =
            SharedPreparation::begin(project, &environment, "checked").unwrap()
        else {
            panic!()
        };
        let SharedPreparation::Join(waiter) =
            SharedPreparation::begin(project, &environment, "checked").unwrap()
        else {
            panic!()
        };
        drop(owner);
        assert!(waiter.recv().unwrap().unwrap_err().contains("interrupted"));
    }
}
