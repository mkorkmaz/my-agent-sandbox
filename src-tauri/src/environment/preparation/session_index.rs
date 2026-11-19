use crate::platform::brig::Brig;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Components, Path, PathBuf};

const MAX_INDEX_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
struct SessionEntry {
    sandbox: Option<String>,
    project: Option<PathBuf>,
}

#[derive(Default)]
pub(super) struct SessionIndex(HashMap<String, SessionEntry>);

impl SessionIndex {
    pub(super) fn read(brig: &Brig) -> Self {
        read_entries(brig).map(Self).unwrap_or_default()
    }

    pub(super) fn excludes(&self, reference: &str, sandbox: &str, selected: &Path) -> bool {
        let Some(entry) = self.0.get(reference) else {
            return false;
        };
        if sandbox.is_empty() || entry.sandbox.as_deref() != Some(sandbox) {
            return false;
        }
        let Some(saved) = entry
            .project
            .as_deref()
            .filter(|path| valid_project_path(path))
        else {
            return false;
        };
        !paths_may_match(saved, selected)
    }
}

fn index_path(brig: &Brig) -> Option<PathBuf> {
    let environment = &brig.environment;
    if let Some(directory) = environment
        .get("BRIG_STATE_DIR")
        .filter(|value| !value.is_empty())
    {
        return Some(Path::new(directory).join("sessions.json"));
    }
    let home = environment.get("HOME").filter(|value| !value.is_empty())?;
    Some(Path::new(home).join(".brig/sessions.json"))
}

fn read_entries(brig: &Brig) -> Option<HashMap<String, SessionEntry>> {
    let input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(index_path(brig)?)
        .ok()?;
    if !input.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    input
        .take(MAX_INDEX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn valid_project_path(path: &Path) -> bool {
    path.is_absolute()
        && path.file_name().is_some()
        && !path.as_os_str().as_encoded_bytes().contains(&0)
        && !path.components().any(|part| part == Component::ParentDir)
}

fn paths_may_match(saved: &Path, selected: &Path) -> bool {
    let saved = comparable_components(saved);
    let selected = comparable_components(selected);
    saved.clone().count() == selected.clone().count()
        && saved
            .zip(selected)
            .all(|(saved, selected)| names_may_match(saved.as_os_str(), selected.as_os_str()))
}

fn comparable_components(path: &Path) -> Components<'_> {
    // Brig can record a path through these macOS system aliases. Compare the
    // names without opening any part of a remembered project path.
    let mut parts = path.components();
    parts.next();
    #[cfg(target_os = "macos")]
    {
        let mut suffix = parts.clone();
        if suffix
            .next()
            .is_some_and(|part| part.as_os_str().eq_ignore_ascii_case("private"))
            && suffix.clone().next().is_some_and(|part| {
                ["etc", "tmp", "var"]
                    .iter()
                    .any(|alias| part.as_os_str().eq_ignore_ascii_case(alias))
            })
        {
            return suffix;
        }
    }
    parts
}

#[cfg(target_os = "macos")]
fn names_may_match(saved: &OsStr, selected: &OsStr) -> bool {
    use core_foundation::base::{CFComparisonResult, TCFType};
    use core_foundation::string::{
        kCFCompareCaseInsensitive, kCFCompareNonliteral, CFString, CFStringCompare,
    };

    if saved == selected {
        return true;
    }
    let Some((saved, selected)) = saved.to_str().zip(selected.to_str()) else {
        return true;
    };
    let saved = CFString::new(saved);
    let selected = CFString::new(selected);
    // The two CFStrings keep the references alive for the duration of this call.
    let comparison = unsafe {
        CFStringCompare(
            saved.as_concrete_TypeRef(),
            selected.as_concrete_TypeRef(),
            kCFCompareCaseInsensitive | kCFCompareNonliteral,
        )
    };
    comparison == CFComparisonResult::EqualTo
}

#[cfg(not(target_os = "macos"))]
fn names_may_match(saved: &OsStr, selected: &OsStr) -> bool {
    saved == selected
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_spellings_can_match_without_opening_their_paths() {
        for (saved, selected) in [
            ("/TMP/CAFÉ", "/private/tmp/Cafe\u{301}"),
            ("/var/Projects/A", "/private/var/projects/a"),
            ("/private/etc/PROJECT", "/etc/project"),
            ("/Volumes/Café/A", "/Volumes/CAFE\u{301}/a"),
        ] {
            assert!(paths_may_match(Path::new(saved), Path::new(selected)));
        }
        for (saved, selected) in [
            ("/Volumes/Café/project", "/Volumes/Café 2/project"),
            ("/var-other/project", "/private/var-other/project"),
            ("/tmp/Project/member", "/private/tmp/Project"),
        ] {
            assert!(!paths_may_match(Path::new(saved), Path::new(selected)));
        }
    }
}
