//! Opt-in release test worker. Every path must be a generated benchmark folder.
#![allow(unexpected_cfgs)]
use super::*;
use serde_json::json;
use std::cell::RefCell;
use std::io::{BufRead, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::Instant;

#[derive(Default)]
struct Timing {
    parser: u128,
    backend: u128,
    parser_depth: usize,
    backend_depth: usize,
    details: BTreeMap<&'static str, u128>,
}
thread_local! {static TIMING:RefCell<Option<Timing>>=const{RefCell::new(None)};}
pub(crate) struct Detail {
    name: &'static str,
    start: Option<Instant>,
}
impl Detail {
    pub(crate) fn new(name: &'static str) -> Self {
        let active = TIMING.with(|timing| timing.borrow().is_some());
        Self {
            name,
            start: active.then(Instant::now),
        }
    }
}
impl Drop for Detail {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };
        TIMING.with(|timing| {
            if let Some(timing) = timing.borrow_mut().as_mut() {
                *timing.details.entry(self.name).or_default() += start.elapsed().as_nanos();
            }
        });
    }
}
pub(crate) struct Phase {
    start: Option<Instant>,
    parser: bool,
}
impl Phase {
    pub(crate) fn parser() -> Self {
        Self::start(true)
    }
    pub(crate) fn backend() -> Self {
        Self::start(false)
    }
    fn start(parser: bool) -> Self {
        let active = TIMING.with(|timing| {
            let mut timing = timing.borrow_mut();
            let Some(timing) = timing.as_mut() else {
                return false;
            };
            let depth = if parser {
                &mut timing.parser_depth
            } else {
                &mut timing.backend_depth
            };
            *depth += 1;
            true
        });
        Self {
            start: active.then(Instant::now),
            parser,
        }
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };
        TIMING.with(|timing| {
            let mut timing = timing.borrow_mut();
            let Some(timing) = timing.as_mut() else {
                return;
            };
            let depth = if self.parser {
                &mut timing.parser_depth
            } else {
                &mut timing.backend_depth
            };
            *depth -= 1;
            if *depth == 0 {
                if self.parser {
                    timing.parser += start.elapsed().as_nanos();
                } else {
                    timing.backend += start.elapsed().as_nanos();
                }
            }
        });
    }
}

const PROJECTS: [&str; 2] = [
    "11111111111111111111111111111111",
    "22222222222222222222222222222222",
];
fn session(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012x}")
}
fn turn(index: usize) -> String {
    format!("10000000-0000-4000-8000-{index:012x}")
}
fn target(project: usize, kind: AgentKind) -> environment_history::Target {
    environment_history::Target {
        project_id: PROJECTS[project].into(),
        environment_id: projects::EnvironmentId::for_agent(PROJECTS[project], kind),
        agent_kind: kind,
    }
}
fn ledger_path(root: &Path, target: &environment_history::Target) -> PathBuf {
    if target.agent_kind == AgentKind::Codex {
        return root
            .join("conversation-ledgers")
            .join(format!("{}.json", target.project_id));
    }
    root.join("conversation-ledgers")
        .join(&target.project_id)
        .join(format!("{}.json", target.environment_id))
}
fn watch(root: &Path, records: &Path, project: usize, kind: AgentKind) -> Watch {
    let target = target(project, kind);
    Watch {
        home: root
            .join("guests")
            .join(PROJECTS[project])
            .join(kind.binary()),
        project: PathBuf::from(format!("/work/benchmark-{project}")),
        ledger: ledger_path(records, &target),
        target: Some(target),
        records_directory: None,
        #[cfg(not(tugboat_json_baseline))]
        store: None,
    }
}
fn native(watch: &Watch, id: &str) -> PathBuf {
    watch
        .home
        .join(".codex/sessions")
        .join(format!("rollout-fixture-{id}.jsonl"))
}
fn exact(id: &str, ordinal: usize) -> Value {
    let count = ordinal + 1;
    let tokens = json!({"input_tokens":100,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":20,"total_tokens":120});
    json!({"type":"token_usage_record","timestamp":format!("2026-11-13T00:00:{:02}.000123Z",ordinal%60),"payload":{"thread_id":id,"turn_id":turn(ordinal),"root_turn_id":turn(ordinal),"response_id":format!("benchmark-response-{id}-{ordinal}"),"usage":tokens,"thread_token_usage":{"input_tokens":100*count,"cached_input_tokens":10*count,"cache_write_input_tokens":0,"output_tokens":20*count,"total_tokens":120*count}}})
}
fn generate_native(watch: &Watch, count: usize) {
    for index in 0..count {
        let id = session(index);
        if watch.target.as_ref().unwrap().agent_kind == AgentKind::Codex {
            let path = native(watch, &id);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let source = if index == 1 || index == 2 {
                json!({"subagent":{"thread_spawn":{"parent_thread_id":session(index-1),"depth":index,"agent_path":format!("/root/child-{index}")}}})
            } else {
                json!("cli")
            };
            let mut lines = vec![
                json!({"type":"session_meta","payload":{"id":id,"cwd":watch.project,"source":source,"cli_version":"0.167.0","model_provider":"openai"}}),
            ];
            for ordinal in 0..8 {
                lines.push(json!({"type":"turn_context","payload":{"turn_id":turn(ordinal),"model":"gpt-6.1-sol","root_turn_id":turn(ordinal)}}));
                lines.push(exact(&id, ordinal));
                lines.push(json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":turn(ordinal)}}));
            }
            fs::write(
                path,
                lines
                    .into_iter()
                    .map(|line| format!("{line}\n"))
                    .collect::<String>(),
            )
            .unwrap();
            continue;
        }
        let path = watch
            .home
            .join(".claude/projects/benchmark")
            .join(format!("{id}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut lines = vec![
            json!({"type":"user","uuid":format!("user-{id}"),"sessionId":id,"cwd":watch.project,"version":"2.1.318","isSidechain":false,"timestamp":"2026-11-13T00:00:00Z","message":{"role":"user","content":"Generated benchmark fixture."}}),
        ];
        for ordinal in 0..8 {
            lines.push(json!({"type":"assistant","uuid":format!("assistant-{id}-{ordinal}"),"sessionId":id,"cwd":watch.project,"version":"2.1.318","isSidechain":false,"timestamp":format!("2026-11-13T00:00:{ordinal:02}Z"),"message":{"id":format!("claude-benchmark-{id}-{ordinal}"),"role":"assistant","model":"claude-opus-5-5","stop_reason":"end_turn","content":[],"usage":{"input_tokens":90,"cache_read_input_tokens":10,"cache_creation_input_tokens":0,"output_tokens":20}}}));
        }
        fs::write(
            path,
            lines
                .into_iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
}
fn copy_ledgers(root: &Path, source: &str, destination: &Path) {
    for project in 0..2 {
        for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
            let target = target(project, kind);
            let path = ledger_path(destination, &target);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::copy(ledger_path(&root.join(source), &target), path).unwrap();
        }
    }
}

struct Worker {
    root: PathBuf,
    records: PathBuf,
    watches: Vec<Watch>,
    history: History,
    #[cfg(not(tugboat_json_baseline))]
    shared: crate::usage_store::SharedStore,
}
impl Worker {
    fn open(root: &Path) -> Self {
        #[cfg(tugboat_json_baseline)]
        let name = "json";
        #[cfg(not(tugboat_json_baseline))]
        let name = "sql";
        let records = root.join(name);
        fs::create_dir_all(&records).unwrap();
        fs::set_permissions(&records, fs::Permissions::from_mode(0o700)).unwrap();
        copy_ledgers(root, "origin", &records);
        let guard = crate::storage::data_location::RecordsDirectory::fixture(&records);
        #[cfg(not(tugboat_json_baseline))]
        let shared = {
            crate::usage_store::importer::activate(&guard).unwrap();
            crate::usage_store::SharedStore::new(crate::usage_store::Store::open(&guard).unwrap())
        };
        let mut watches = Vec::new();
        for project in 0..2 {
            for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
                let mut item = watch(root, &records, project, kind);
                item.records_directory = Some(guard.clone());
                #[cfg(not(tugboat_json_baseline))]
                {
                    item.store = Some(shared.clone());
                }
                watches.push(item);
            }
        }
        let history = History::default();
        for (index, watch) in watches.iter().enumerate() {
            history
                .watches
                .lock()
                .unwrap()
                .insert(format!("benchmark-{index}"), watch.clone());
        }
        let worker = Self {
            root: root.into(),
            records,
            watches,
            history,
            #[cfg(not(tugboat_json_baseline))]
            shared,
        };
        worker.history.flush().unwrap();
        worker
    }
    fn dashboard(&self) -> Dashboard {
        let dashboards = self.watches[..2].iter().map(|watch| {
            #[cfg(tugboat_json_baseline)]
            {
                let ledger = load_ledger(&watch.ledger).unwrap();
                let mut dashboard = dashboard_from_ledger(ledger, HashMap::new()).unwrap();
                watch.target.as_ref().unwrap().annotate(&mut dashboard);
                dashboard
            }
            #[cfg(not(tugboat_json_baseline))]
            {
                sql_backend::dashboard(watch.target.as_ref().unwrap(), &self.shared, false).unwrap()
            }
        });
        environment_history::combine(dashboards)
    }
    fn details(&self) -> ConversationUsageDetails {
        let watch = &self.watches[0];
        #[cfg(tugboat_json_baseline)]
        {
            let ledger = watch.selected_ledger().unwrap();
            usage_details::from_ledger(&ledger, &session(0)).unwrap()
        }
        #[cfg(not(tugboat_json_baseline))]
        {
            sql_backend::details(
                watch.target.as_ref().unwrap(),
                &self.shared,
                &session(0),
                |_| Ok(Some(watch.namespace())),
            )
            .unwrap()
        }
    }
    fn perform(&self, op: &str, step: usize) -> Value {
        match op {
            "status" => {
                let state = if step % 2 == 0 {
                    SessionState::Archived
                } else {
                    SessionState::Active
                };
                change_state(&self.watches[0], &session(0), state, |_| false).unwrap();
                json!({"state":state})
            }
            "details" => {
                let details = self.details();
                std::hint::black_box(&details);
                json!({"requests":details.combined_usage.requests,"agents":details.agents.len(),"costUsd":details.combined_usage.cost_usd})
            }
            "dashboard" => {
                let dashboard = self.dashboard();
                std::hint::black_box(&dashboard);
                json!({"requests":dashboard.usage.requests,"sessions":dashboard.conversations.len(),"costUsd":dashboard.usage.cost_usd})
            }
            "dashboard_live" => {
                let mut dashboards = Vec::new();
                for watch in &self.watches[..2] {
                    dashboards.push(synchronize(watch).unwrap().0);
                }
                let dashboard = environment_history::combine(dashboards.into_iter());
                json!({"requests":dashboard.usage.requests,"sessions":dashboard.conversations.len()})
            }
            "quiet" => {
                self.history.flush().unwrap();
                json!({"environments":self.watches.len()})
            }
            "append" => {
                let (dashboard, changed) = synchronize(&self.watches[0]).unwrap();
                assert!(changed);
                json!({"requests":dashboard.usage.requests,"changed":changed})
            }
            _ => panic!("Unknown benchmark operation"),
        }
    }
    fn append_source(&self, step: usize) {
        let id = session(0);
        let ordinal = 8 + step;
        let mut file = OpenOptions::new()
            .append(true)
            .open(native(&self.watches[0], &id))
            .unwrap();
        writeln!(file,"{}",json!({"type":"turn_context","payload":{"turn_id":turn(ordinal),"model":"gpt-6.1-sol","root_turn_id":turn(ordinal)}})).unwrap();
        writeln!(file, "{}", exact(&id, ordinal)).unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":turn(ordinal)}})
        )
        .unwrap();
        file.sync_all().unwrap();
    }
    #[cfg(not(tugboat_json_baseline))]
    fn migration(&self, step: usize) -> Value {
        let path = self.root.join(format!("migration-{step}"));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        copy_ledgers(&self.root, "origin", &path);
        let environment = target(0, AgentKind::ClaudeCode);
        let file = ledger_path(&path, &environment);
        let mut ledger = load_ledger(&file).unwrap();
        for record in ledger.sessions.values_mut() {
            for request in record
                .turns
                .values_mut()
                .flat_map(|turn| &mut turn.requests)
            {
                request.anthropic = None;
                request.rates = None;
                request.cost_usd = None;
                request.anthropic_corrections.clear();
                request.unpriced_reason = Some("Generated legacy unpriced fixture".into());
            }
        }
        storage::write(&file, &ledger).unwrap();
        let guard = crate::storage::data_location::RecordsDirectory::fixture(&path);
        let start = Instant::now();
        crate::usage_store::importer::activate(&guard).unwrap();
        let import = start.elapsed().as_nanos();
        let start = Instant::now();
        let mut store = crate::usage_store::Store::open(&guard).unwrap();
        let open = start.elapsed().as_nanos();
        let environment = environment.usage_environment().unwrap();
        let start = Instant::now();
        assert!(store.prepare_claude(&environment).unwrap());
        let prepare = start.elapsed().as_nanos();
        let changes = store.connection.total_changes();
        assert!(!store.prepare_claude(&environment).unwrap());
        assert_eq!(changes, store.connection.total_changes());
        let disk = fs::metadata(path.join("usage.sqlite3")).unwrap().len();
        store.close().unwrap();
        fs::remove_dir_all(path).unwrap();
        json!({"importNs":import,"prepareNs":prepare,"openNs":open,"databaseBytes":disk})
    }
    #[cfg(not(tugboat_json_baseline))]
    fn changes(&self) -> u64 {
        self.shared
            .with(|store| Ok(store.connection.total_changes()))
            .unwrap()
    }
    #[cfg(not(tugboat_json_baseline))]
    fn steady_open(&self) -> Value {
        self.shared.close().unwrap();
        let records = crate::storage::data_location::RecordsDirectory::fixture_existing(
            &self.records,
            &"a".repeat(32),
        );
        let environment = target(0, AgentKind::ClaudeCode)
            .usage_environment()
            .unwrap();
        crate::usage_store::record_reads::begin();
        let start = Instant::now();
        let mut store = crate::usage_store::Store::open(&records).unwrap();
        assert!(!store.prepare_claude(&environment).unwrap());
        let elapsed = start.elapsed().as_nanos();
        let records_loaded = crate::usage_store::record_reads::finish();
        assert!(records_loaded.is_empty());
        let changes = store.connection.total_changes();
        assert_eq!(changes, 0);
        store.close().unwrap();
        json!({"steadyOpenNs":elapsed,"rawRecordsLoaded":records_loaded.len(),"dataChanges":changes,"policySweepSkipped":true})
    }
    #[cfg(not(tugboat_json_baseline))]
    fn prepare_copy(&self, step: usize) -> Value {
        let template = self.root.join("unprepared-template");
        if !template.exists() {
            fs::create_dir(&template).unwrap();
            fs::set_permissions(&template, fs::Permissions::from_mode(0o700)).unwrap();
            copy_ledgers(&self.root, "origin", &template);
            let environment = target(0, AgentKind::ClaudeCode);
            let file = ledger_path(&template, &environment);
            let mut ledger = load_ledger(&file).unwrap();
            for record in ledger.sessions.values_mut() {
                for request in record
                    .turns
                    .values_mut()
                    .flat_map(|turn| &mut turn.requests)
                {
                    request.anthropic = None;
                    request.rates = None;
                    request.cost_usd = None;
                    request.anthropic_corrections.clear();
                    request.unpriced_reason = Some("Generated legacy unpriced fixture".into());
                }
            }
            storage::write(&file, &ledger).unwrap();
            let records = crate::storage::data_location::RecordsDirectory::fixture(&template);
            crate::usage_store::importer::activate(&records).unwrap();
            crate::usage_store::Store::open(&records)
                .unwrap()
                .close()
                .unwrap();
        }
        let path = self.root.join(format!("first-prep-{step}"));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        // The template was closed and synchronized before either file is copied.
        for name in ["usage.sqlite3", ".tugboat-data.json"] {
            fs::copy(template.join(name), path.join(name)).unwrap();
        }
        let records = crate::storage::data_location::RecordsDirectory::fixture_existing(
            &path,
            &"a".repeat(32),
        );
        let mut store = crate::usage_store::Store::open(&records).unwrap();
        let environment = target(0, AgentKind::ClaudeCode)
            .usage_environment()
            .unwrap();
        let start = Instant::now();
        assert!(store.prepare_claude(&environment).unwrap());
        let elapsed = start.elapsed().as_nanos();
        let policy_versions:Vec<(String,i64)>=store.connection.prepare("SELECT project_id,claude_pricing_version FROM usage_environments WHERE agent_kind='claude-code' ORDER BY project_id").unwrap().query_map([],|row|Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(
            policy_versions,
            vec![(PROJECTS[0].into(), 1), (PROJECTS[1].into(), 0)]
        );
        store.close().unwrap();
        let mut reopened = crate::usage_store::Store::open(&records).unwrap();
        let changes = reopened.connection.total_changes();
        assert!(!reopened.prepare_claude(&environment).unwrap());
        assert_eq!(reopened.connection.total_changes(), changes);
        reopened.close().unwrap();
        fs::remove_dir_all(path).unwrap();
        json!({"firstPreparationNs":elapsed,"policyVersions":policy_versions,"reopenSkipped":true})
    }
    #[cfg(not(tugboat_json_baseline))]
    fn diagnostics(&self) -> Value {
        unsafe extern "C" fn deny_facts(
            _context: *mut std::ffi::c_void,
            action: std::ffi::c_int,
            table: *const std::ffi::c_char,
            _column: *const std::ffi::c_char,
            _database: *const std::ffi::c_char,
            _trigger: *const std::ffi::c_char,
        ) -> std::ffi::c_int {
            if table.is_null() {
                return rusqlite::ffi::SQLITE_OK;
            }
            let table = unsafe { std::ffi::CStr::from_ptr(table) }.to_bytes();
            if [
                rusqlite::ffi::SQLITE_READ,
                rusqlite::ffi::SQLITE_UPDATE,
                rusqlite::ffi::SQLITE_INSERT,
                rusqlite::ffi::SQLITE_DELETE,
            ]
            .contains(&action)
                && [b"usage_requests".as_slice(), b"source_cursors".as_slice()].contains(&table)
            {
                return rusqlite::ffi::SQLITE_DENY;
            }
            rusqlite::ffi::SQLITE_OK
        }
        // Establish a known state before checking the one-row opposite transition.
        self.perform("status", 0);
        self.shared
            .with(|store| {
                unsafe {
                    rusqlite::ffi::sqlite3_set_authorizer(
                        store.connection.handle(),
                        Some(deny_facts),
                        std::ptr::null_mut(),
                    );
                }
                Ok(())
            })
            .unwrap();
        let before = self.changes();
        self.perform("status", 1);
        let status_changes = self.changes() - before;
        self.dashboard();
        self.shared
            .with(|store| {
                unsafe {
                    rusqlite::ffi::sqlite3_set_authorizer(
                        store.connection.handle(),
                        None,
                        std::ptr::null_mut(),
                    );
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(status_changes, 1);
        self.shared.with(|store| {
            let version:(String,String)=store.connection.query_row("SELECT sqlite_version(),sqlite_source_id()",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
            let mut pragmas=BTreeMap::new();for pragma in ["foreign_keys","journal_mode","synchronous","fullfsync","trusted_schema","busy_timeout"] {let value=store.connection.query_row(&format!("PRAGMA {pragma}"),[],|row|Ok(match row.get_ref(0)?{rusqlite::types::ValueRef::Integer(value)=>json!(value),rusqlite::types::ValueRef::Text(value)=>json!(String::from_utf8_lossy(value)),_=>Value::Null})).unwrap();pragmas.insert(pragma,value);}
            let mut plans=Vec::new();for query in ["SELECT * FROM usage_environments WHERE project_id=?1","SELECT * FROM session_groups WHERE environment_key=?1 AND native_session_id=?2","SELECT * FROM usage_requests WHERE source_guest=?1 AND model_provider=?2 AND response_id=?3 AND origin_thread_id=?4"] {let count=query.matches('?').count();let values=[PROJECTS[0],"openai","response","origin"];let rows=store.connection.prepare(&format!("EXPLAIN QUERY PLAN {query}")).unwrap().query_map(rusqlite::params_from_iter(values[..count].iter()),|row|row.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();plans.push(json!({"query":query,"plan":rows}));}
            let counts:(i64,i64)=store.connection.query_row("SELECT (SELECT count(*) FROM session_records),(SELECT count(*) FROM usage_requests)",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
            Ok(json!({"sqliteVersion":version.0,"sqliteSourceId":version.1,"pragmas":pragmas,"queryPlans":plans,"records":counts.0,"requests":counts.1,"statusDataChanges":status_changes,"statusAndDashboardDenyRawFacts":true}))
        }).unwrap()
    }
    fn storage_bytes(&self) -> u64 {
        fn size(path: &Path) -> u64 {
            let metadata = fs::symlink_metadata(path).unwrap();
            if metadata.is_file() {
                return metadata.len();
            }
            if !metadata.is_dir() {
                return 0;
            }
            fs::read_dir(path)
                .unwrap()
                .map(|entry| size(&entry.unwrap().path()))
                .sum()
        }
        size(&self.records)
    }
}

fn emit(value: Value) {
    println!("TUGBOAT_BENCH {value}");
    std::io::stdout().flush().unwrap();
}

#[test]
#[ignore = "Generated fixtures only; invoked by scripts/benchmark-sqlite-usage.py"]
fn worker() {
    let root = PathBuf::from(
        std::env::var("TUGBOAT_BENCH_ROOT").expect("explicit generated benchmark root"),
    );
    let root = fs::canonicalize(root).unwrap();
    assert!(root.join(".generated-tugboat-benchmark").is_file());
    if std::env::var("TUGBOAT_BENCH_GENERATE").as_deref() == Ok("1") {
        let sessions: usize = std::env::var("TUGBOAT_BENCH_SESSIONS")
            .unwrap()
            .parse()
            .unwrap();
        assert!([10, 1000].contains(&sessions));
        let origin = root.join("origin");
        fs::create_dir_all(&origin).unwrap();
        let mut counts = Vec::new();
        for project in 0..2 {
            for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
                let item = watch(&root, &origin, project, kind);
                generate_native(&item, sessions / 2);
                let (dashboard, _) = synchronize(&item).unwrap();
                assert_eq!(dashboard.usage.requests, (sessions / 2 * 8) as u64);
                counts.push(json!({"project":project,"kind":kind,"sessions":dashboard.conversations.len(),"requests":dashboard.usage.requests}));
            }
        }
        emit(
            json!({"generated":counts,"device":fs::metadata(&root).unwrap().dev(),"inode":fs::metadata(&root).unwrap().ino()}),
        );
        return;
    }
    let worker = Worker::open(&root);
    emit(
        json!({"ready":true,"storageBytes":worker.storage_bytes(),"device":fs::metadata(&worker.records).unwrap().dev()}),
    );
    for command in std::io::stdin().lock().lines() {
        let command: Value = serde_json::from_str(&command.unwrap()).unwrap();
        let op = command["op"].as_str().unwrap();
        let step = command["step"].as_u64().unwrap_or(0) as usize;
        if op == "stop" {
            break;
        }
        if op == "append-source" {
            worker.append_source(step);
            emit(json!({"appended":step}));
            continue;
        }
        #[cfg(not(tugboat_json_baseline))]
        if op == "migration" {
            emit(worker.migration(step));
            continue;
        }
        #[cfg(not(tugboat_json_baseline))]
        if op == "diagnostics" {
            emit(worker.diagnostics());
            continue;
        }
        #[cfg(not(tugboat_json_baseline))]
        if op == "steady_open" {
            emit(worker.steady_open());
            continue;
        }
        #[cfg(not(tugboat_json_baseline))]
        if op == "prepare_copy" {
            emit(worker.prepare_copy(step));
            continue;
        }
        #[cfg(not(tugboat_json_baseline))]
        let before = worker.changes();
        #[cfg(not(tugboat_json_baseline))]
        crate::usage_store::record_reads::begin();
        TIMING.with(|timing| *timing.borrow_mut() = Some(Timing::default()));
        let start = Instant::now();
        let output = worker.perform(op, step);
        let elapsed = start.elapsed().as_nanos();
        let timing = TIMING.with(|timing| timing.borrow_mut().take().unwrap());
        #[cfg(not(tugboat_json_baseline))]
        let records_loaded = crate::usage_store::record_reads::finish();
        #[cfg(not(tugboat_json_baseline))]
        if op == "status" || op == "dashboard" {
            assert!(records_loaded.is_empty());
        }
        #[cfg(not(tugboat_json_baseline))]
        if op == "details" {
            assert_eq!(records_loaded.len(), 3);
            worker.shared.with(|store|{for record in &records_loaded {let identity:(String,String,String)=store.connection.query_row("SELECT e.project_id,e.agent_kind,g.native_session_id FROM session_records r JOIN session_groups g ON g.session_key=r.session_key JOIN usage_environments e ON e.environment_key=g.environment_key WHERE r.record_id=?1",[record],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();assert_eq!(identity.0,PROJECTS[0]);assert_eq!(identity.1,"codex");assert!([session(0),session(1),session(2)].contains(&identity.2));}Ok(())}).unwrap();
        }
        #[cfg(not(tugboat_json_baseline))]
        let changes = worker.changes() - before;
        #[cfg(tugboat_json_baseline)]
        let changes: Option<u64> = None;
        emit(
            json!({"op":op,"step":step,"endToEndNs":elapsed,"guestParseNs":timing.parser,"backendNs":timing.backend,"otherNs":elapsed.saturating_sub(timing.parser+timing.backend),"detailNs":timing.details,"dataChanges":changes,"result":output}),
        );
    }
}
