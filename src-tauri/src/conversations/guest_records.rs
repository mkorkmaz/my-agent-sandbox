// The public session ID is a Codex UUID. When it occurs in multiple guests,
// sessions contains the selected guest's row and variants retains the other rows.
use super::{guest_namespace, Ledger, SessionRecord};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

fn source_guest(path: &Path) -> Option<String> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return None;
    }
    let directory = path
        .ancestors()
        .find(|path| path.file_name().is_some_and(|part| part == ".codex"))?;
    directory.parent().map(guest_namespace)
}

impl Ledger {
    pub(super) fn select_guest(&mut self, guest: &str) -> bool {
        let mut changed = false;
        // Upgrade old rows using source evidence. Mixed rows predate per-guest state.
        for (id, record) in &mut self.sessions {
            if record.guest_namespace.is_some() {
                continue;
            }
            let owners: BTreeSet<String> = record
                .cursors
                .iter()
                .filter_map(|(path, cursor)| {
                    cursor
                        .guest_namespace()
                        .map(str::to_owned)
                        .or_else(|| source_guest(path))
                })
                .chain(
                    record
                        .turns
                        .values()
                        .flat_map(|turn| &turn.requests)
                        .filter_map(|request| request.source.as_ref())
                        .map(|source| source.guest_namespace.clone()),
                )
                .filter(|owner| !owner.is_empty())
                .collect();
            if owners.len() == 1 {
                record.guest_namespace = owners.into_iter().next();
                changed = true;
            } else if owners.len() > 1 {
                let variants = self.variants.entry(id.clone()).or_default();
                for (path, cursor) in std::mem::take(&mut record.cursors) {
                    if let Some(owner) = cursor
                        .guest_namespace()
                        .map(str::to_owned)
                        .or_else(|| source_guest(&path))
                    {
                        let part = variants.entry(owner.clone()).or_default();
                        part.guest_namespace = Some(owner);
                        part.parser_version = record.parser_version;
                        part.cursors.insert(path, cursor);
                    } else {
                        record.cursors.insert(path, cursor);
                    }
                }
                record.turns.retain(|key, turn| {
                    let was_empty = turn.requests.is_empty();
                    let requests = std::mem::take(&mut turn.requests);
                    for request in requests {
                        if let Some(source) = request
                            .source
                            .as_ref()
                            .filter(|source| !source.guest_namespace.is_empty())
                        {
                            let owner = source.guest_namespace.clone();
                            let part = variants.entry(owner.clone()).or_default();
                            part.guest_namespace = Some(owner);
                            part.turns
                                .entry(key.clone())
                                .or_insert_with(|| {
                                    let mut metadata = turn.clone();
                                    metadata.requests.clear();
                                    metadata
                                })
                                .requests
                                .push(request);
                        } else {
                            turn.requests.push(request);
                        }
                    }
                    was_empty || !turn.requests.is_empty()
                });
                changed = true;
            }
        }
        for (id, variants) in &mut self.variants {
            if self
                .sessions
                .get(id)
                .and_then(|row| row.guest_namespace.as_deref())
                == Some(guest)
            {
                continue;
            }
            if let Some(previous) = self.sessions.remove(id) {
                variants.insert(
                    previous.guest_namespace.clone().unwrap_or_default(),
                    previous,
                );
                changed = true;
            }
            if let Some(selected) = variants.remove(guest) {
                self.sessions.insert(id.clone(), selected);
                changed = true;
            }
        }
        changed
    }

    pub(super) fn admit_guest(&mut self, id: &str, guest: &str) {
        if self
            .sessions
            .get(id)
            .is_some_and(|row| row.guest_namespace.as_deref() != Some(guest))
        {
            let previous = self.sessions.remove(id).expect("existing row");
            self.variants.entry(id.to_owned()).or_default().insert(
                previous.guest_namespace.clone().unwrap_or_default(),
                previous,
            );
        }
        let row = self.sessions.entry(id.to_owned()).or_default();
        row.guest_namespace = Some(guest.to_owned());
    }
}

pub(super) type Variants = BTreeMap<String, BTreeMap<String, SessionRecord>>;
