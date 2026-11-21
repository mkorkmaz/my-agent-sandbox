use crate::projects::{Project, PREVIOUS_ENVIRONMENT_IMAGE};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FileIdentity {
    device: u64,
    inode: u64,
}

impl From<&fs::Metadata> for FileIdentity {
    fn from(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RootEvidence {
    pub(super) disk: FileIdentity,
    pub(super) fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NamedHome {
    pub(super) path: PathBuf,
    identity: FileIdentity,
}

impl NamedHome {
    pub(super) fn read(path: &Path) -> Result<Self, String> {
        let metadata =
            fs::symlink_metadata(path).map_err(|_| "The persistent guest home is unavailable.")?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || fs::canonicalize(path).ok().as_deref() != Some(path)
        {
            return Err(
                "The persistent guest home changed. Review the project before continuing.".into(),
            );
        }
        Ok(Self {
            path: path.into(),
            identity: (&metadata).into(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Change {
    pub(super) path: String,
    pub(super) kind: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scope {
    pub(super) reference: String,
    pub(super) project: PathBuf,
    pub(super) home: NamedHome,
    pub(super) plan_fingerprint: String,
}

impl Scope {
    fn verify(&self, project: &Project) -> Result<(), String> {
        if self.reference != project.reference()
            || self.project != project.path
            || !self.home.path.is_absolute()
            || !hex_digest(&self.plan_fingerprint)
        {
            return Err("The guest image recovery record does not match this project.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Review {
    pub(super) scope: Scope,
    pub(super) root: RootEvidence,
}

impl Review {
    pub(super) fn token(&self) -> Result<String, String> {
        let value = serde_json::to_value(self).map_err(|_| "Could not record the image review.")?;
        crate::platform::brig::fingerprint(&value)
    }
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "phase",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum Pending {
    Reviewing {
        scope: Scope,
    },
    Reviewed {
        review: Review,
        backup: RootEvidence,
    },
    Applying {
        review: Review,
        backup: RootEvidence,
    },
    Finalizing {
        review: Review,
        backup: RootEvidence,
    },
}

impl Pending {
    pub(crate) fn validate(&self, project: &Project) -> Result<(), String> {
        let scope = match self {
            Self::Reviewing { scope } => scope,
            Self::Reviewed { review, backup }
            | Self::Applying { review, backup }
            | Self::Finalizing { review, backup } => {
                if !hex_digest(&review.root.fingerprint)
                    || review.root.fingerprint != backup.fingerprint
                {
                    return Err(
                        "The guest image recovery record has an invalid system disk.".into(),
                    );
                }
                &review.scope
            }
        };
        scope.verify(project)?;
        if !matches!(self, Self::Finalizing { .. })
            && project.image_ref != PREVIOUS_ENVIRONMENT_IMAGE
        {
            return Err("The guest image recovery record has an invalid source image.".into());
        }
        Ok(())
    }
}
