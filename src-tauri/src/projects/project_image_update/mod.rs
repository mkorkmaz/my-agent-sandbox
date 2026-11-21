use crate::{
    desktop::settings,
    environment::{
        self,
        shared_runtime::{pinned_versions, Versions},
        EnvironmentOperation,
    },
    platform::runtime as tools,
    projects::{
        self, EnvironmentAttempt, EnvironmentId, Project, ProjectStage, Registry,
        PREVIOUS_ENVIRONMENT_IMAGE,
    },
    storage::{self, data_location},
    terminal,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::AppHandle;

#[cfg(test)]
mod admission_tests;
#[cfg(all(test, target_os = "macos"))]
mod live;
#[cfg(all(test, target_os = "macos"))]
mod post_update;
#[cfg(test)]
pub(crate) mod proof_policy;
mod replay;
mod root;
mod runtime;
mod state;

pub(crate) use super::DEFAULT_ENVIRONMENT_IMAGE as CANDIDATE_IMAGE;
use runtime::{scope, Runtime};
pub(crate) use state::Pending;
use state::{Change, Review, RootEvidence, Scope};

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Action {
    Read,
    Review,
    Apply { token: String },
    KeepCurrent { token: String },
    Recover,
}

#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
enum Status {
    Available {
        previous: &'static str,
        candidate: &'static str,
        previous_versions: Versions,
        candidate_versions: Versions,
    },
    Current {
        image: &'static str,
        versions: Versions,
    },
    Review {
        previous: &'static str,
        candidate: &'static str,
        changes: Vec<Change>,
        token: String,
        previous_versions: Versions,
        candidate_versions: Versions,
    },
    Recovery {
        previous: &'static str,
        candidate: &'static str,
        message: String,
    },
    Updated {
        previous: &'static str,
        candidate: &'static str,
        versions: Versions,
    },
    Restored {
        image: &'static str,
        versions: Versions,
        message: String,
    },
    Recovered,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    project_id: String,
    environment_id: EnvironmentId,
    #[serde(flatten)]
    status: Status,
}

fn idle(project: &Project) -> Result<(), String> {
    if project.removal_pending()
        || !matches!(
            project.stage,
            ProjectStage::Ready | ProjectStage::EnvironmentReady | ProjectStage::UpdatingImage
        )
    {
        return Err("Finish project setup or the current project operation before updating the guest image.".into());
    }
    if terminal::terminal_sessions(project.id.clone())
        .iter()
        .any(|terminal| terminal.running)
    {
        return Err(
            "Close the project's active chats and terminals before updating the guest image."
                .into(),
        );
    }
    Ok(())
}

fn runtime<'a>(app: &AppHandle, project: &'a Project) -> Result<Runtime<'a>, String> {
    tools::require_root_restore()?;
    let settings = project.settings(settings::load(&settings::file(app)?)?)?;
    let brig = environment::brig_for_project(app, project, &settings)?;
    let hull = brig
        .environment
        .get("BRIG_RUNTIME_BIN")
        .map(std::path::PathBuf::from)
        .ok_or("The bundled Hull is unavailable.")?;
    let cosign = brig
        .environment
        .get("BRIG_COSIGN_BIN")
        .map(std::path::PathBuf::from)
        .or_else(|| tools::tool("cosign"))
        .ok_or("The bundled image verifier is unavailable.")?;
    Ok(Runtime {
        brig,
        project,
        settings,
        protected: data_location::protected_directories(app)?,
        hull,
        cosign,
        debugfs: tools::tool("debugfs").ok_or("The bundled system disk reader is unavailable.")?,
        e2fsck: tools::tool("e2fsck")
            .ok_or("The bundled filesystem recovery tool is unavailable.")?,
    })
}

fn current_scope(runtime: &Runtime<'_>) -> Result<(environment::CheckedProject, Scope), String> {
    let (previous, home) = runtime.plan(PREVIOUS_ENVIRONMENT_IMAGE)?;
    let (candidate, new_home) = runtime.plan(CANDIDATE_IMAGE)?;
    let plan_fingerprint = scope(&previous.plan)?;
    if home != new_home || plan_fingerprint != scope(&candidate.plan)? {
        return Err("The image would change the project's home, mount, policy, or preview.".into());
    }
    super::operations::verify_target(&runtime.brig, runtime.project)?;
    Ok((
        previous,
        Scope {
            reference: runtime.project.reference(),
            project: runtime.project.path.clone(),
            home,
            plan_fingerprint,
        },
    ))
}

fn protect_review(
    file: &Path,
    runtime: &Runtime<'_>,
    expected: &Scope,
    preparing: &Pending,
) -> Result<(Project, Review, RootEvidence), String> {
    let (previous, current) = current_scope(runtime)?;
    if &current != expected {
        return Err(
            "The project plan changed. Restore the reviewed plan before continuing.".into(),
        );
    }
    let first = runtime.instance(&previous)?;
    if first.status == "running" {
        runtime.flush(&previous)?;
        runtime.stop()?;
    }
    let instance = runtime.instance(&previous)?;
    let upper = runtime.upper(&instance)?;
    let review = Review {
        scope: current,
        root: root::evidence(&upper)?,
    };
    let backup = root::backup(&upper, &runtime.backup_path()?, &review.root)?;
    let saved = save(file, runtime.project, Some(preparing), |project| {
        project.image_update = Some(Pending::Reviewed {
            review: review.clone(),
            backup: backup.clone(),
        });
        Ok(())
    })?;
    Ok((saved, review, backup))
}

fn review_view(
    runtime: &Runtime<'_>,
    review: &Review,
    backup: &RootEvidence,
) -> Result<Status, String> {
    let path = runtime.backup_path()?;
    if root::evidence(&path)? != *backup {
        return Err("The reviewed guest system disk backup changed.".into());
    }
    let replay = runtime.replay(false)?;
    let changes = root::inventory(&runtime.debugfs, &replay, &runtime.brig.environment)?;
    Ok(Status::Review {
        previous: PREVIOUS_ENVIRONMENT_IMAGE,
        candidate: CANDIDATE_IMAGE,
        changes,
        token: review.token()?,
        previous_versions: pinned_versions(PREVIOUS_ENVIRONMENT_IMAGE)?,
        candidate_versions: pinned_versions(CANDIDATE_IMAGE)?,
    })
}

fn save(
    file: &Path,
    target: &Project,
    expected: Option<&Pending>,
    update: impl FnOnce(&mut Project) -> Result<(), String>,
) -> Result<Project, String> {
    let _lock = storage::lock(file)?;
    let mut registry = Registry::load(file)?;
    let mut project = registry.environment(&target.id, &target.environment_id)?;
    if project.reference() != target.reference()
        || project.path != target.path
        || project.image_update.as_ref() != expected
        || project.image_ref != target.image_ref
        || project.removal_pending()
    {
        return Err("The registered project changed. Review its image and try again.".into());
    }
    project.check_path()?;
    update(&mut project)?;
    registry.update(project.clone())?;
    storage::write(file, &registry)?;
    Ok(project)
}

fn boot_target(
    runtime: &Runtime<'_>,
    review: &Review,
    image: &str,
    restore: bool,
) -> Result<(environment::CheckedProject, Versions), String> {
    let (checked, home) = runtime.plan(image)?;
    if review.scope.home != home || review.scope.plan_fingerprint != scope(&checked.plan)? {
        return Err("The reviewed guest home, mount, policy, or preview changed.".into());
    }
    let versions = runtime.boot(&checked, restore)?;
    let (effective, effective_home) = runtime.plan(image)?;
    if home != effective_home || review.scope.plan_fingerprint != scope(&effective.plan)? {
        return Err("The replacement guest changed the reviewed project plan.".into());
    }
    Ok((effective, versions))
}

fn finalize(
    file: &Path,
    runtime: &Runtime<'_>,
    pending: &Pending,
    checked: &environment::CheckedProject,
) -> Result<(), String> {
    let (review, backup) = backup_review(pending)?;
    let finalizing = Pending::Finalizing {
        review: review.clone(),
        backup: backup.clone(),
    };
    let project = save(file, runtime.project, Some(pending), |project| {
        project.image_ref = checked.project.image_ref.clone();
        project.image_update = Some(finalizing.clone());
        let mut attempt = EnvironmentAttempt::pending(
            "verified-guest-image".into(),
            0,
            checked.fingerprint.clone(),
        );
        attempt.finish(Ok(()))?;
        project.environment_attempt = Some(attempt);
        project.approved_plan = Some(checked.fingerprint.clone());
        project.environment_verified = Some(true);
        Ok(())
    })?;
    replay::cleanup(&runtime.backup_path()?, backup)?;
    root::clear_backup(&runtime.backup_path()?, backup)?;
    save(file, &project, Some(&finalizing), |project| {
        project.image_update = None;
        project.stage = ProjectStage::EnvironmentReady;
        Ok(())
    })?;
    Ok(())
}

fn restore(
    file: &Path,
    runtime: &Runtime<'_>,
    pending: &Pending,
    message: String,
) -> Result<Status, String> {
    let (review, backup) = backup_review(pending)?;
    if root::evidence(&runtime.backup_path()?)? != *backup {
        return Err(
            "The previous system disk backup changed. Recovery stopped before using it.".into(),
        );
    }
    let (checked, current_home) = runtime.plan(PREVIOUS_ENVIRONMENT_IMAGE)?;
    if current_home != review.scope.home || scope(&checked.plan)? != review.scope.plan_fingerprint {
        return Err(
            "Restore the reviewed home, mount, policy, and preview before retrying recovery."
                .into(),
        );
    }
    let replay = runtime.replay(true)?;
    runtime.stop()?;
    let (checked, versions) = boot_target(runtime, review, PREVIOUS_ENVIRONMENT_IMAGE, true)?;
    drop(replay);
    finalize(file, runtime, pending, &checked)?;
    Ok(Status::Restored {
        image: PREVIOUS_ENVIRONMENT_IMAGE,
        versions,
        message,
    })
}

fn backup_review(pending: &Pending) -> Result<(&Review, &RootEvidence), String> {
    match pending {
        Pending::Reviewed { review, backup } | Pending::Applying { review, backup } => {
            Ok((review, backup))
        }
        _ => Err("The guest image file review is incomplete.".into()),
    }
}

fn replace(
    file: &Path,
    runtime: &Runtime<'_>,
    token: &str,
    use_candidate: bool,
    candidate: impl FnOnce(
        &Runtime<'_>,
        &Review,
    ) -> Result<(environment::CheckedProject, Versions), String>,
) -> Result<Status, String> {
    let saved = runtime
        .project
        .image_update
        .as_ref()
        .ok_or("Check the guest image before replacing it.")?;
    let Pending::Reviewed { review, backup } = saved else {
        return Err("The guest image review is incomplete.".into());
    };
    if review.token()? != token || root::evidence(&runtime.backup_path()?)? != *backup {
        return Err("The guest image review changed. Check the image again.".into());
    }
    let (previous, current) = current_scope(runtime)?;
    if current != review.scope
        || root::evidence(&runtime.upper(&runtime.instance(&previous)?)?)? != review.root
    {
        return Err(
            "The stopped guest or the reviewed plan changed. Retry recovery before replacing it."
                .into(),
        );
    }
    let pending = Pending::Applying {
        review: review.clone(),
        backup: backup.clone(),
    };
    save(file, runtime.project, Some(saved), |project| {
        project.image_update = Some(pending.clone());
        Ok(())
    })?;
    if !use_candidate {
        return restore(
            file,
            runtime,
            &pending,
            "Kept the previous guest image and its system files.".into(),
        );
    }
    match candidate(runtime, review) {
        Ok((checked, versions)) => {
            finalize(file, runtime, &pending, &checked)?;
            Ok(Status::Updated { previous: PREVIOUS_ENVIRONMENT_IMAGE, candidate: CANDIDATE_IMAGE, versions })
        }
        Err(message) => restore(file, runtime, &pending, message).map_err(|error| {
            format!("The new image did not start successfully. Retry recovery from Project settings. {error}")
        }),
    }
}

fn pending_action(
    file: &Path,
    runtime: &Runtime<'_>,
    pending: &Pending,
    action: Action,
) -> Result<Status, String> {
    match (pending, action) {
        (Pending::Reviewed { review, backup }, Action::Read | Action::Review) => {
            Ok(review_view(runtime, review, backup).unwrap_or_else(recovery))
        }
        (Pending::Reviewed { .. }, Action::Apply { token }) => {
            replace(file, runtime, &token, true, |runtime, review| {
                boot_target(runtime, review, CANDIDATE_IMAGE, false)
            })
        }
        (Pending::Reviewed { .. }, Action::KeepCurrent { token }) => {
            replace(file, runtime, &token, false, |_, _| unreachable!())
        }
        (Pending::Reviewing { scope }, Action::Recover) => {
            let (saved, _, _) = protect_review(file, runtime, scope, pending)?;
            let local = Runtime {
                project: &saved,
                ..runtime.clone()
            };
            let Some(pending) = saved.image_update.as_ref() else {
                return Err("The previous guest image recovery state is unavailable.".into());
            };
            restore(
                file,
                &local,
                pending,
                "Recovered an interrupted guest image review.".into(),
            )
        }
        (Pending::Reviewed { .. } | Pending::Applying { .. }, Action::Recover) => restore(
            file,
            runtime,
            pending,
            "Recovered an interrupted guest image change.".into(),
        ),
        (Pending::Finalizing { backup, .. }, Action::Recover) => {
            replay::cleanup(&runtime.backup_path()?, backup)?;
            root::clear_backup(&runtime.backup_path()?, backup)?;
            save(file, runtime.project, Some(pending), |project| {
                project.image_update = None;
                project.stage = ProjectStage::EnvironmentReady;
                Ok(())
            })?;
            Ok(Status::Recovered)
        }
        _ => Ok(recovery(
            "Continue guest image recovery for this project.".into(),
        )),
    }
}

fn recovery(message: String) -> Status {
    Status::Recovery {
        previous: PREVIOUS_ENVIRONMENT_IMAGE,
        candidate: CANDIDATE_IMAGE,
        message,
    }
}

fn change(file: &Path, runtime: &Runtime<'_>, action: Action) -> Result<Status, String> {
    idle(runtime.project)?;
    if let Some(pending) = &runtime.project.image_update {
        return pending_action(file, runtime, pending, action);
    }
    if runtime.project.image_ref == CANDIDATE_IMAGE {
        let (checked, _) = runtime.plan(CANDIDATE_IMAGE)?;
        runtime.instance(&checked)?;
        return Ok(Status::Current {
            image: CANDIDATE_IMAGE,
            versions: pinned_versions(CANDIDATE_IMAGE)?,
        });
    }
    match action {
        Action::Read => Ok(Status::Available {
            previous: PREVIOUS_ENVIRONMENT_IMAGE,
            candidate: CANDIDATE_IMAGE,
            previous_versions: pinned_versions(PREVIOUS_ENVIRONMENT_IMAGE)?,
            candidate_versions: pinned_versions(CANDIDATE_IMAGE)?,
        }),
        Action::Review => {
            let (previous, scope) = current_scope(runtime)?;
            runtime.instance(&previous)?;
            runtime.stage(CANDIDATE_IMAGE)?;
            idle(runtime.project)?;
            let preparing = Pending::Reviewing {
                scope: scope.clone(),
            };
            let saved = save(file, runtime.project, None, |project| {
                project.image_update = Some(preparing.clone());
                project.stage = ProjectStage::UpdatingImage;
                Ok(())
            })?;
            let local = Runtime {
                project: &saved,
                ..runtime.clone()
            };
            let (_, review, backup) = protect_review(file, &local, &scope, &preparing)?;
            review_view(runtime, &review, &backup)
        }
        _ => Err("Check the guest image before changing it.".into()),
    }
}

#[tauri::command]
pub(crate) async fn project_image_update(
    app: AppHandle,
    identifier: String,
    environment_id: EnvironmentId,
    action: Action,
) -> Result<View, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = EnvironmentOperation::begin(&identifier, &environment_id)?;
        let file = projects::file(&app)?;
        let registry = Registry::load(&file)?;
        data_location::validate_project_records(&app, &registry)?;
        let project = registry.environment(&identifier, &environment_id)?;
        project.check_path()?;
        project.check_guest_path()?;
        let status = change(&file, &runtime(&app, &project)?, action)?;
        Ok(View {
            project_id: identifier,
            environment_id,
            status,
        })
    })
    .await
    .map_err(|_| "Could not complete the guest image operation.")?
}
