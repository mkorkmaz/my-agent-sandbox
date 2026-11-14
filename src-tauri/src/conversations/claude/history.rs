use crate::conversations::*;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::ffi::OsStrExt,
};

pub(in crate::conversations) fn open_at(
    parent: &File,
    name: &OsStr,
    directory: bool,
) -> Result<Option<File>, String> {
    let name = CString::new(name.as_bytes()).map_err(|_| "Invalid Claude history entry.")?;
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
    let flags = flags | if directory { libc::O_DIRECTORY } else { 0 };
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor >= 0 {
        return Ok(Some(unsafe { File::from_raw_fd(descriptor) }));
    }
    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
        return Ok(None);
    }
    Err(
        "Could not safely open Claude history. Symbolic links and special files are unsupported."
            .into(),
    )
}

fn register(
    parsed: Result<Option<claude::metadata::Transcript>, String>,
    path: PathBuf,
    scan: &mut HistoryScan,
    sessions: &mut BTreeMap<String, claude::metadata::Transcript>,
    duplicates: &mut BTreeSet<String>,
) {
    match parsed {
        Ok(Some(transcript)) => {
            let id = transcript.session.id.clone();
            if sessions.insert(id.clone(), transcript).is_some() {
                duplicates.insert(id);
            }
        }
        Ok(None) => {}
        Err(message) => {
            scan.problems.insert(path, message);
        }
    }
}

struct Discovery<'a> {
    namespace: &'a str,
    project: &'a Path,
    count: usize,
    scan: HistoryScan,
    sessions: BTreeMap<String, claude::metadata::Transcript>,
    duplicates: BTreeSet<String>,
}

impl Discovery<'_> {
    fn count(&mut self) -> Result<(), String> {
        self.count += 1;
        if self.count > 65_536 {
            return Err("The Claude transcript list exceeds the supported size.".into());
        }
        Ok(())
    }

    fn transcript(&mut self, directory: &File, path: PathBuf, root: &str, agent: Option<&str>) {
        let parsed = open_at(
            directory,
            path.file_name().expect("transcript filename"),
            false,
        )
        .and_then(|file| match file {
            Some(file) => claude::metadata::read(
                file,
                path.clone(),
                root,
                self.project,
                self.namespace,
                agent,
            ),
            None => Ok(None),
        })
        .map(|transcript| {
            transcript.map(|mut transcript| {
                if let Some(agent) = agent {
                    transcript.parent = claude::subagents::metadata(directory, agent, root);
                }
                transcript
            })
        });
        register(
            parsed,
            path,
            &mut self.scan,
            &mut self.sessions,
            &mut self.duplicates,
        );
    }

    fn children(
        &mut self,
        directory: &File,
        path: &Path,
        root: &str,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 8 {
            return Err("The Claude subagent directory exceeds the supported depth.".into());
        }
        for entry in
            fs::read_dir(path).map_err(|_| "Could not read Claude subagent transcripts.")?
        {
            self.count()?;
            let entry = entry.map_err(|_| "Could not read a Claude subagent entry.")?;
            let kind = entry
                .file_type()
                .map_err(|_| "Could not verify a Claude subagent entry.")?;
            if kind.is_symlink() {
                self.scan
                    .warnings
                    .push("A symbolic link in Claude subagent history was skipped.".into());
                continue;
            }
            if kind.is_dir() {
                let child = open_at(directory, &entry.file_name(), true)?
                    .ok_or("The Claude subagent directory moved.")?;
                self.children(&child, &entry.path(), root, depth + 1)?;
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(agent) = name
                .strip_prefix("agent-")
                .and_then(|name| name.strip_suffix(".jsonl"))
                .filter(|agent| claude::subagents::valid_agent_id(agent))
            {
                self.transcript(directory, entry.path(), root, Some(agent));
            }
        }
        Ok(())
    }

    fn project(&mut self, directory: &File, path: &Path) -> Result<(), String> {
        for entry in fs::read_dir(path).map_err(|_| "Could not read Claude transcripts.")? {
            self.count()?;
            let entry = entry.map_err(|_| "Could not read a Claude transcript entry.")?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(root) = name.strip_suffix(".jsonl").filter(|id| valid_id(id)) {
                self.transcript(directory, entry.path(), root, None);
                continue;
            }
            if !valid_id(&name) {
                continue;
            }
            let children = (|| {
                let Some(session) = open_at(directory, &entry.file_name(), true)? else {
                    return Ok(());
                };
                let Some(subagents) = open_at(&session, OsStr::new("subagents"), true)? else {
                    return Ok(());
                };
                self.children(&subagents, &entry.path().join("subagents"), &name, 0)
            })();
            if let Err(message) = children {
                self.scan.problems.insert(entry.path(), message);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(in crate::conversations) fn scan(home: &Path, project: &Path) -> Result<HistoryScan, String> {
    scan_known(home, project, &BTreeMap::new())
}

fn scan_known(
    home: &Path,
    project: &Path,
    known: &BTreeMap<String, SessionRecord>,
) -> Result<HistoryScan, String> {
    #[cfg(test)]
    let _benchmark_phase = crate::conversations::sqlite_benchmark::Phase::parser();
    if !home
        .join(".claude")
        .try_exists()
        .map_err(|_| "Could not read the Claude guest home.")?
    {
        return Ok(HistoryScan::default());
    }
    let anchor = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(home)
        .map_err(|_| "The Claude guest home is unavailable.")?;
    let namespace = guest_namespace(
        &fs::canonicalize(home).map_err(|_| "The Claude guest home is unavailable.")?,
    );
    let configuration = open_at(&anchor, OsStr::new(".claude"), true)?
        .ok_or("The Claude history directory moved. Refresh its list.")?;
    let Some(projects) = open_at(&configuration, OsStr::new("projects"), true)? else {
        return Ok(HistoryScan::default());
    };
    let mut discovery = Discovery {
        namespace: &namespace,
        project,
        count: 0,
        scan: HistoryScan::default(),
        sessions: BTreeMap::new(),
        duplicates: BTreeSet::new(),
    };
    for (count, entry) in fs::read_dir(home.join(".claude/projects"))
        .map_err(|_| "Could not read the Claude projects.")?
        .enumerate()
    {
        if count >= 4096 {
            return Err("The Claude project list exceeds the supported size.".into());
        }
        let entry = entry.map_err(|_| "Could not read a Claude project entry.")?;
        let kind = entry
            .file_type()
            .map_err(|_| "Could not verify a Claude project entry.")?;
        if kind.is_symlink() {
            discovery
                .scan
                .warnings
                .push("A symbolic link in Claude history was skipped.".into());
            continue;
        }
        if !kind.is_dir() {
            continue;
        }
        if let Some(directory) = open_at(&projects, &entry.file_name(), true)? {
            discovery.project(&directory, &entry.path())?;
        }
    }
    for id in &discovery.duplicates {
        discovery.sessions.remove(id);
        discovery
            .scan
            .lineage_problems
            .insert(id.clone(), lineage::Problem::Conflicting);
        discovery.scan.warnings.push("Multiple Claude transcripts use the same session ID or agent ID. Known usage is retained; the source is unavailable.".into());
    }
    claude::subagents::resolve(
        &mut discovery.scan,
        &mut discovery.sessions,
        known,
        home,
        project,
        &namespace,
    );
    for (id, transcript) in discovery.sessions {
        discovery.scan.turns.insert(id.clone(), transcript.turns);
        discovery.scan.claude_sources.insert(id, transcript.source);
        discovery.scan.sessions.push(transcript.session);
    }
    if !discovery.scan.problems.is_empty() {
        discovery.scan.warnings.push("Some Claude chats or subagents could not be read. Their last saved entries remain available.".into());
    }
    discovery.scan.warnings.sort();
    discovery.scan.warnings.dedup();
    Ok(discovery.scan)
}

pub(in crate::conversations) fn synchronize(watch: &Watch) -> Result<(Dashboard, bool), String> {
    watch.check_storage()?;
    let _guard = watch.legacy_lock()?;
    let (mut ledger, mut scan_state) = watch.scan_ledger()?;
    let namespace = watch.namespace();
    scan_state.prepare_selection(watch, &mut ledger, &namespace)?;
    let mut changed = ledger.select_guest(&namespace) || ledger.schema_version != 2;
    let mut scan = scan_known(&watch.home, &watch.project, &ledger.sessions)?;
    let mut needed: BTreeSet<_> = scan
        .sessions
        .iter()
        .map(|session| session.id.clone())
        .collect();
    let roots: BTreeSet<_> = scan
        .sessions
        .iter()
        .map(|session| {
            scan.claude_sources
                .get(&session.id)
                .map(|source| source.root_session_id.clone())
                .unwrap_or_else(|| session.id.clone())
        })
        .collect();
    for candidate in watch.claude_owner_ids(&scan)? {
        if ledger.sessions.get(&candidate).is_some_and(|record| {
            let root = record
                .claude_source
                .as_ref()
                .map(|source| source.root_session_id.as_str())
                .unwrap_or(&candidate);
            record.guest_namespace.as_deref() == Some(&namespace) && roots.contains(root)
        }) {
            needed.insert(candidate);
        }
    }
    for (id, record) in &ledger.sessions {
        let warning =
            "The saved Claude transcript is unavailable. Refresh after its history is restored.";
        if record.source_state() == SourceState::Available
            || record.warning.as_deref() != Some(warning)
            || record.agent_kind != Some(AgentKind::ClaudeCode)
            || scan
                .lineage_problems
                .get(id)
                .is_some_and(|problem| record.lineage_problem.as_ref() != Some(problem))
            || scan
                .exclusions
                .get(id)
                .is_some_and(|exclusion| record.project_exclusion.as_ref() != Some(exclusion))
        {
            needed.insert(id.clone());
        }
    }
    scan_state.hydrate(watch, &mut ledger, needed, Some(&namespace))?;
    let known_turns = ledger
        .sessions
        .iter()
        .flat_map(|(id, record)| {
            record
                .turns
                .iter()
                .filter(|(_, turn)| !turn.requests.is_empty())
                .map(|(turn, _)| (id.clone(), turn.clone()))
        })
        .collect();
    changed |= ledger.warnings != scan.warnings;
    ledger.warnings = scan.warnings;
    let mut sources = HashMap::new();
    for session in scan.sessions {
        if let Some(record) = ledger.sessions.get_mut(&session.id).filter(|record| {
            record.guest_namespace.is_none()
                && record.cursors.is_empty()
                && record.turns.values().all(|turn| turn.requests.is_empty())
        }) {
            record.guest_namespace = Some(namespace.clone());
            changed = true;
        }
        ledger.admit_guest(&session.id, &namespace);
        let record = ledger
            .sessions
            .get_mut(&session.id)
            .expect("admitted Claude chat");
        let previous_turns = record.turns.clone();
        let conflict = claude::usage::merge(
            &mut record.turns,
            scan.turns.remove(&session.id).unwrap_or_default(),
        );
        let warning = if conflict {
            Some(
                "Conflicting Claude message usage was skipped. Known recorded usage is retained."
                    .into(),
            )
        } else {
            session.warning.clone()
        };
        changed |= record.agent_kind != Some(AgentKind::ClaudeCode);
        claude::usage::prepare(record);
        changed |= previous_turns != record.turns;
        changed |= record.observe_source(SourceState::Available)
            || record.title != session.title
            || record.updated_at != session.updated_at
            || record.lineage != session.lineage
            || record.lineage_problem != session.lineage_problem
            || record.source_root_id != session.source_root_id
            || record.warning != warning;
        record.title = session.title.clone();
        record.updated_at = session.updated_at;
        record.lineage = session.lineage.clone();
        record.lineage_problem = session.lineage_problem.clone();
        record.source_root_id = session.source_root_id.clone();
        let source = scan.claude_sources.remove(&session.id);
        changed |= record.claude_source != source;
        record.claude_source = source;
        if scan.positive_members.contains(&session.id) {
            changed |= record.project_exclusion.take().is_some();
        }
        record.warning = warning;
        if let Some(problem) = &record.lineage_problem {
            record.warning = Some(
                record
                    .warning
                    .take()
                    .map(|warning| format!("{warning} {}", problem.message()))
                    .unwrap_or_else(|| problem.message().into()),
            );
        }
        sources.insert(session.id.clone(), session);
    }
    for (id, record) in &mut ledger.sessions {
        if let Some(problem) = scan.lineage_problems.remove(id) {
            changed |= record.lineage_problem.as_ref() != Some(&problem);
            record.lineage_problem = Some(problem);
        }
        if let Some(exclusion) = scan.exclusions.remove(id) {
            changed |= record.project_exclusion.as_ref() != Some(&exclusion);
            record.project_exclusion = Some(exclusion);
        }
        if scan_state.is_loaded(id) {
            changed |= record.agent_kind != Some(AgentKind::ClaudeCode) || record.turns.is_empty();
            claude::usage::prepare(record);
        }
        if sources.contains_key(id) {
            continue;
        }
        changed |= record.observe_source(SourceState::Unavailable);
        let warning =
            "The saved Claude transcript is unavailable. Refresh after its history is restored.";
        changed |= record.warning.as_deref() != Some(warning);
        record.warning = Some(warning.into());
    }
    changed |= claude::usage::deduplicate(&mut ledger, &known_turns);
    scan_state.finish(watch, ledger, sources, changed)
}
