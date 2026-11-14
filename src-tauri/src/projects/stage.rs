use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectStage {
    Selected,
    CreatingPolicy,
    PolicyCreated,
    AttachingPolicy,
    EditingPolicy,
    PolicyDetached,
    Ready,
    EnvironmentUnverified,
    EnvironmentStarting,
    EnvironmentReady,
    Stopping,
    Removing,
}

impl ProjectStage {
    pub fn permits_plan(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::EnvironmentStarting | Self::EnvironmentReady
        )
    }

    pub fn invalidate_preparation(self) -> Self {
        if matches!(self, Self::EnvironmentStarting | Self::EnvironmentReady) {
            return Self::Ready;
        }
        self
    }
}

#[cfg(test)]
impl PartialEq<&str> for ProjectStage {
    fn eq(&self, other: &&str) -> bool {
        serde_json::to_value(self).unwrap().as_str() == Some(*other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_stage_values_remain_compatible_and_unknown_stages_fail() {
        for name in [
            "selected",
            "creating-policy",
            "policy-created",
            "attaching-policy",
            "editing-policy",
            "policy-detached",
            "ready",
            "environment-unverified",
            "environment-starting",
            "environment-ready",
            "stopping",
            "removing",
        ] {
            let stage: ProjectStage = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert_eq!(serde_json::to_value(stage).unwrap(), name);
        }
        assert!(
            serde_json::from_value::<ProjectStage>(serde_json::json!("unexpected-stage")).is_err()
        );
    }

    #[test]
    fn invalidation_preserves_setup_detachment_and_removal_checkpoints() {
        assert_eq!(
            ProjectStage::EnvironmentReady.invalidate_preparation(),
            ProjectStage::Ready
        );
        assert_eq!(
            ProjectStage::EnvironmentStarting.invalidate_preparation(),
            ProjectStage::Ready
        );
        for stage in [
            ProjectStage::CreatingPolicy,
            ProjectStage::PolicyDetached,
            ProjectStage::Removing,
        ] {
            assert_eq!(stage.invalidate_preparation(), stage);
            assert!(!stage.permits_plan());
        }
    }
}
