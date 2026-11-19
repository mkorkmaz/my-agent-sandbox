use super::{runner, CODEX, REFERENCE};
use crate::{
    platform::{command, runtime},
    providers::{shared_chatgpt, shared_claude},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    env, fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

mod chat;

const PROFILE: &[u8] = include_bytes!("../../../../scripts/acceptance/shared-project-profile.yaml");
const POLICY: &[u8] = include_bytes!("../../../../scripts/acceptance/shared-project-policy.yaml");
const CLAUDE_CREDENTIALS: &str = include_str!("../../providers/shared_claude/guest_credentials.py");
const CODEX_DIGEST: &str = "2fe94d848540ff2a6868c7ebcc494126a974aa409711676d14fa0379e49e6143";
const SANDBOX: &str = "brig-tugboat-project-proof-proof";

fn image_reference() -> &'static str {
    std::str::from_utf8(PROFILE)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("image: "))
        .unwrap()
}

struct Guest {
    executable: PathBuf,
    environment: BTreeMap<String, String>,
}

impl Guest {
    fn from_root(root: &Path) -> Self {
        runtime::validate().expect("The pinned bundled runtime did not pass validation");
        let directory = fs::canonicalize(runtime::directory().unwrap()).unwrap();
        let username = env::var("USER").expect("The host user is unavailable");
        let inside = |path| root.join(path).to_str().unwrap().to_owned();
        let environment = BTreeMap::from([
            ("HOME".into(), inside("home")),
            ("USER".into(), username.clone()),
            ("LOGNAME".into(), username),
            (
                "PATH".into(),
                format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", directory.display()),
            ),
            ("LANG".into(), "en_US.UTF-8".into()),
            ("TERM".into(), "xterm-256color".into()),
            ("TMPDIR".into(), inside("tmp")),
            ("XDG_CONFIG_HOME".into(), inside("config")),
            ("DOCKER_CONFIG".into(), inside("registry")),
            ("BRIG_STATE_DIR".into(), inside("state")),
            ("BRIG_PROFILE_DIR".into(), inside("profiles")),
            ("BRIG_POLICY_DIR".into(), inside("policies")),
            ("BRIG_RUNTIME".into(), "hull".into()),
            (
                "BRIG_RUNTIME_BIN".into(),
                directory.join("hull").to_str().unwrap().into(),
            ),
            ("BRIG_VERIFY".into(), "require".into()),
            ("DO_NOT_TRACK".into(), "1".into()),
            (
                "MKE2FS_CONFIG".into(),
                directory.join("../mke2fs.conf").to_str().unwrap().into(),
            ),
        ]);
        Self {
            executable: directory.join("brig"),
            environment,
        }
    }

    fn run_tool(
        &self,
        tool: &Path,
        arguments: &[&str],
        input: Option<Vec<u8>>,
    ) -> command::CommandOutput {
        let arguments = arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>();
        command::run_with_input(
            tool,
            &arguments,
            None,
            &self.environment,
            Duration::from_secs(25),
            &AtomicBool::new(false),
            input,
        )
        .expect("The disposable guest command could not start")
    }

    fn run(&self, arguments: &[&str], input: Option<Vec<u8>>) -> command::CommandOutput {
        self.run_tool(&self.executable, arguments, input)
    }

    fn json(&self, arguments: &[&str], kind: &str) -> Value {
        let output = self.run(arguments, None);
        assert!(
            output.exit_code == Some(0),
            "Could not verify the disposable guest"
        );
        let value: Value = serde_json::from_str(&output.stdout).expect("Invalid Brig response");
        assert_eq!(value["apiVersion"], "brig.sh/v1alpha1");
        assert_eq!(value["kind"], kind);
        value["data"].clone()
    }

    fn credentials(&self, operation: &str, input: Option<Vec<u8>>) -> bool {
        self.run(
            &[
                "exec",
                REFERENCE,
                "--",
                "python3",
                "-c",
                CLAUDE_CREDENTIALS,
                operation,
            ],
            input,
        )
        .exit_code
            == Some(0)
    }

    fn running_state(&self) -> Option<Value> {
        let output = self.run_tool(
            &self.executable.with_file_name("hull"),
            &["inspect", SANDBOX],
            None,
        );
        if output.exit_code != Some(0) {
            return None;
        }
        serde_json::from_str(&output.stdout).ok()
    }
}

fn expected_environment(root: &Path, plan: &Value) -> bool {
    let allowed = [
        "api.anthropic.com",
        "api.openai.com",
        "auth.openai.com",
        "chatgpt.com",
        "claude.ai",
        "platform.claude.com",
    ];
    plan["refused"].is_null()
        && plan["profile"] == "tugboat-project-proof"
        && plan["session"] == "proof"
        && plan["sandbox"] == SANDBOX
        && plan["runtime"]["backend"] == "hvi"
        && plan["posture"] == "isolated"
        && plan["verify"] == json!({"mode":"require", "policy":"brig"})
        && plan["policies"] == json!(["tugboat-project-proof"])
        && plan["egress"]
            == json!({"default":"deny", "allow":allowed.map(|host| json!({"host":host})), "deny":[]})
        && plan["ports"] == json!([])
        && plan["credentials"] == json!([])
        && plan["image"]["ref"] == image_reference()
        && plan["project"]
            == json!({"host":root.join("sample project"),"guest":"/work/sample project","mode":"read-write"})
        && plan["home"]
            == json!({"host":root.join(format!("state/homes/{SANDBOX}")),"guest":"/root","mode":"read-write"})
}

fn check_files(root: &Path) {
    let metadata = fs::symlink_metadata(root).unwrap();
    assert!(metadata.is_dir() && metadata.uid() == unsafe { libc::geteuid() });
    assert_eq!(metadata.mode() & 0o777, 0o700);
    for (file, expected) in [
        ("profiles/tugboat-project-proof.yaml", PROFILE),
        ("policies/tugboat-project-proof.yaml", POLICY),
        ("registry/config.json", b"{}\n"),
    ] {
        let file = root.join(file);
        assert_eq!(fs::canonicalize(&file).unwrap(), file);
        assert!(fs::symlink_metadata(&file).unwrap().is_file());
        assert!(
            fs::read(&file).unwrap() == expected,
            "A proof input changed"
        );
    }
}

fn checked_plan(root: &Path, guest: &Guest) -> PathBuf {
    check_files(root);
    let project = root.join("sample project");
    let plan = guest.json(
        &[
            "--json",
            "plan",
            "--network",
            "isolated",
            REFERENCE,
            project.to_str().unwrap(),
        ],
        "Plan",
    );
    assert!(
        expected_environment(root, &plan),
        "The disposable guest plan changed"
    );
    let inventory = guest.json(&["--json", "ls"], "SandboxList");
    let sandboxes = inventory["sandboxes"].as_array().unwrap();
    assert_eq!(sandboxes.len(), 1);
    assert_eq!(sandboxes[0]["ref"], REFERENCE);
    assert_eq!(sandboxes[0]["state"], "running");
    let state = guest
        .running_state()
        .expect("Could not check the running Hull instance");
    assert!(
        state["status"] == "running"
            && state["backend"] == "hvi"
            && state["imageDigest"] == image_reference().split_once('@').unwrap().1,
        "The running guest image or backend changed"
    );
    let probe = guest.run(&["exec", REFERENCE, "--", "sh", "-ec", "test \"$(stat -f -c %T \"$CLAUDE_CONFIG_DIR\")\" = tmpfs; test \"$(wc -l < /proc/swaps)\" = 1; sha256sum \"$1\"", "phase1", CODEX], None);
    assert!(
        probe.exit_code == Some(0) && probe.stdout.starts_with(CODEX_DIGEST),
        "The running guest did not verify the executable and memory-only credential path"
    );
    let guest_home = root.join(format!("state/homes/{SANDBOX}"));
    assert_eq!(fs::canonicalize(&guest_home).unwrap(), guest_home);
    guest_home
}

fn provider_record(records: &Path, provider: &str) -> Value {
    let directory = records.join(provider);
    assert_eq!(fs::canonicalize(&directory).unwrap(), directory);
    let bytes = shared_chatgpt::read_private(&directory.join("session.json"))
        .expect("Could not read the designated Dev provider record")
        .expect("The designated Dev provider record is absent");
    serde_json::from_slice(&bytes).expect("Unsupported Dev provider record")
}

fn current_access() -> (Value, Value) {
    let records = env::home_dir()
        .unwrap()
        .join("Library/Application Support/Tugboat Dev");
    let approved = env::var_os("TUGBOAT_PHASE1_LIVE_RECORDS").map(PathBuf::from);
    assert_eq!(approved.as_deref(), Some(records.as_path()));
    assert_eq!(fs::canonicalize(&records).unwrap(), records);
    let chatgpt = provider_record(&records, "chatgpt");
    let claude = provider_record(&records, "claude");
    assert!(chatgpt["disconnected"] == false && chatgpt["reconnectRequired"] == false);
    assert!(claude["account"]["state"] == "connected");
    let chatgpt = shared_chatgpt::Credentials::parse(chatgpt["credentials"].clone(), true).unwrap();
    let claude: shared_claude::Credentials =
        serde_json::from_value(claude["account"]["credentials"].clone())
            .unwrap_or_else(|_| panic!("The designated Claude provider record is unsupported"));
    claude.validate().unwrap();
    let now = chrono::Utc::now();
    assert!(
        chatgpt.expires_at().unwrap() > now.timestamp() + 180,
        "Refresh the designated ChatGPT connection through Dev"
    );
    assert!(
        claude.expires_at > now.timestamp_millis() + 180_000,
        "Refresh the designated Claude connection through Dev"
    );
    let chatgpt = chatgpt.guest();
    let claude = claude.guest();
    assert!(chatgpt["tokens"]["refresh_token"] == "");
    assert!(claude["claudeAiOauth"]["refreshToken"].is_null());
    (chatgpt, claude)
}

struct AccessCopies<'a> {
    guest: &'a Guest,
    codex_auth: PathBuf,
}

impl AccessCopies<'_> {
    fn remove(&self) -> bool {
        let codex = match fs::remove_file(&self.codex_auth) {
            Ok(()) => true,
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        };
        let claude =
            self.guest
                .running_state()
                .is_some_and(|state| match state["status"].as_str() {
                    Some("running") => self.guest.credentials("remove", None),
                    Some("stopped") => true,
                    _ => false,
                });
        codex && claude
    }
}

impl Drop for AccessCopies<'_> {
    fn drop(&mut self) {
        if !self.remove() {
            eprintln!("Disposable access cleanup needs a verified guest stop.");
        }
    }
}

#[test]
fn missing_image_or_modified_policy_cannot_authorize_live_access() {
    let root = Path::new("/private/tmp/tgp1.fixture");
    let mut plan = json!({
        "profile":"tugboat-project-proof", "session":"proof", "sandbox":SANDBOX,
        "runtime":{"backend":"hvi"}, "posture":"isolated", "verify":{"mode":"require", "policy":"brig"},
        "policies":["tugboat-project-proof"], "egress":{"default":"deny", "allow": [
            {"host":"api.anthropic.com"}, {"host":"api.openai.com"}, {"host":"auth.openai.com"},
            {"host":"chatgpt.com"}, {"host":"claude.ai"}, {"host":"platform.claude.com"}
        ], "deny":[]}, "ports":[], "credentials":[],
        "image":{"ref":"ghcr.io/brig-sh/claude-code-stock@sha256:f5b733fbce5aaf38edcdcfe7021993e65625c0f064334a96089e9f1fb5e8d0ce"},
        "project":{"host":root.join("sample project"),"guest":"/work/sample project","mode":"read-write"},
        "home":{"host":root.join(format!("state/homes/{SANDBOX}")),"guest":"/root","mode":"read-write"},
    });
    assert!(expected_environment(root, &plan));
    let original = plan.clone();
    plan["image"] = Value::Null;
    assert!(!expected_environment(root, &plan));
    plan = original;
    plan["egress"]["allow"]
        .as_array_mut()
        .unwrap()
        .push(json!({"host":"example.com"}));
    assert!(!expected_environment(root, &plan));
}

fn publish_access<'a>(guest: &'a Guest, guest_home: &Path) -> AccessCopies<'a> {
    let (chatgpt, claude) = current_access();
    let directory = guest_home.join(".codex");
    if !directory.exists() {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
    }
    assert_eq!(fs::canonicalize(&directory).unwrap(), directory);
    let codex_auth = directory.join("auth.json");
    assert!(fs::symlink_metadata(&codex_auth)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound));
    let copies = AccessCopies { guest, codex_auth };
    crate::storage::write(&copies.codex_auth, &chatgpt).unwrap();
    assert!(
        guest.credentials("write", Some(serde_json::to_vec(&claude).unwrap())),
        "Could not publish the Claude access copy"
    );
    copies
}

fn require_provider_status(guest: &Guest) {
    let codex_status = guest.run(
        &[
            "exec",
            REFERENCE,
            "--",
            CODEX,
            "-c",
            "cli_auth_credentials_store=\"file\"",
            "login",
            "status",
        ],
        None,
    );
    assert!(
        codex_status.exit_code == Some(0)
            && format!("{}{}", codex_status.stdout, codex_status.stderr)
                .to_lowercase()
                .contains("chatgpt"),
        "Codex could not read the ChatGPT access copy"
    );
    let claude_status = guest.run(
        &[
            "exec", REFERENCE, "--", "claude", "auth", "status", "--json",
        ],
        None,
    );
    let response: Value =
        serde_json::from_str(&claude_status.stdout).expect("Claude returned an unsupported status");
    assert!(
        claude_status.exit_code == Some(0) && response["loggedIn"] == true,
        "Claude could not read its access copy"
    );
}

#[test]
#[ignore = "requires authorization to use the designated live Tugboat Dev providers"]
fn published_guest_access_uses_the_intended_providers() {
    let runner = runner();
    let root = runner.parent().unwrap();
    let guest = Guest::from_root(root);
    let guest_home = checked_plan(root, &guest);
    let copies = publish_access(&guest, &guest_home);
    require_provider_status(&guest);
    assert!(
        copies.remove(),
        "Could not remove the disposable access copies"
    );
}
