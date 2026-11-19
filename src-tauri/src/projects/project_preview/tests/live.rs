use super::*;
use std::{
    env,
    io::{Read, Write},
    net::TcpStream,
    os::unix::fs::MetadataExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(in crate::projects) const PROOF_POLICY: &str = "tugboat-project-proof";
const RECEIPT: &str = "Tugboat local preview for proof project A";
const SERVER: &str = r#"import http.server, sys
class Handler(http.server.BaseHTTPRequestHandler):
    def do_HEAD(self):
        self.send_response(200)
        self.end_headers()
    def do_GET(self):
        self.do_HEAD()
        self.wfile.write(sys.argv[2].encode())
http.server.HTTPServer(('0.0.0.0', int(sys.argv[1])), Handler).serve_forever()
"#;

pub(in crate::projects) struct Proof {
    pub(in crate::projects) brig: Brig,
    pub(in crate::projects) settings: settings::Settings,
    pub(in crate::projects) file: PathBuf,
}

impl Proof {
    pub(in crate::projects) fn new(root: &Path, file: PathBuf) -> Self {
        let temporary = Path::new("/tmp").canonicalize().unwrap();
        let metadata = fs::symlink_metadata(root).unwrap();
        assert!(
            root != temporary
                && root.starts_with(&temporary)
                && root.canonicalize().unwrap() == root
        );
        assert!(
            metadata.is_dir()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0
        );
        let path = |name: &str| {
            let directory = root.join(name);
            assert!(directory.is_dir() && directory.canonicalize().unwrap() == directory);
            directory.to_str().unwrap().to_owned()
        };
        let runtime = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/brig-runtime/bin")
            .canonicalize()
            .unwrap();
        let brig = Brig {
            executable: runtime.join("brig"),
            environment: [
                ("HOME".into(), path("home")),
                ("BRIG_STATE_DIR".into(), path("state")),
                ("BRIG_PROFILE_DIR".into(), path("profiles")),
                ("BRIG_POLICY_DIR".into(), path("policies")),
                ("XDG_CONFIG_HOME".into(), path("config")),
                ("DOCKER_CONFIG".into(), path("registry")),
                ("TMPDIR".into(), format!("{}/", path("tmp"))),
                (
                    "PATH".into(),
                    format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", runtime.display()),
                ),
                (
                    "BRIG_RUNTIME_BIN".into(),
                    runtime.join("hull").to_str().unwrap().into(),
                ),
                (
                    "MKE2FS_CONFIG".into(),
                    runtime
                        .parent()
                        .unwrap()
                        .join("mke2fs.conf")
                        .to_str()
                        .unwrap()
                        .into(),
                ),
                ("BRIG_RUNTIME".into(), "hull".into()),
                ("BRIG_HYPERVISOR".into(), "hvi".into()),
                ("BRIG_VERIFY".into(), "require".into()),
                ("DO_NOT_TRACK".into(), "1".into()),
                ("TERM".into(), "dumb".into()),
            ]
            .into(),
        };
        Self {
            brig,
            settings: settings::Settings::default(),
            file,
        }
    }

    fn dispatch(
        &self,
        target: &Project,
        action: Action,
        mapping: Option<Mapping>,
    ) -> Result<View, String> {
        let mut registry = Registry::load(&self.file)?;
        let project = registry.environment(&target.id, &target.environment_id)?;
        project.check_path()?;
        let _operation =
            environment::EnvironmentOperation::begin(&project.id, &project.environment_id)?;
        change(
            &mut registry,
            &self.file,
            &project,
            &self.settings,
            &self.brig,
            action,
            mapping,
        )
    }

    fn server(&self, project: &Project, port: u16) -> Server {
        let child = Command::new(&self.brig.executable)
            .args([
                "exec",
                &project.reference(),
                "--",
                "python3",
                "-c",
                SERVER,
                &port.to_string(),
                RECEIPT,
            ])
            .env_clear()
            .envs(&self.brig.environment)
            .current_dir(&project.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Server(child)
    }

    pub(in crate::projects) fn boot_id(&self, project: &Project) -> String {
        let inventory = self.inventory();
        let rows = inventory["data"]["sandboxes"].as_array().unwrap();
        let reference = project.reference();
        let selected = rows.iter().find(|row| row["ref"] == reference).unwrap();
        assert_eq!(selected["state"], "running");
        assert_eq!(
            selected["workspace"].as_str(),
            self.brig.shared_project_home(project).unwrap().to_str()
        );
        let sandbox = selected["sandbox"].as_str().unwrap();
        assert_eq!(
            sandbox,
            format!("brig-{}-{}", project.profile, project.label)
        );
        let hull = Path::new(&self.brig.environment["BRIG_RUNTIME_BIN"]);
        let args = [
            "exec",
            sandbox,
            "--",
            "cat",
            "/proc/sys/kernel/random/boot_id",
        ]
        .map(str::to_owned);
        let output = crate::platform::command::run(
            hull,
            &args,
            None,
            &self.brig.environment,
            Duration::from_secs(15),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(output.exit_code, Some(0));
        let boot = output.stdout.trim();
        assert_eq!(boot.len(), 36);
        boot.to_owned()
    }

    pub(in crate::projects) fn stop(&self, project: &Project) {
        self.brig
            .checked(&["stop".into(), project.reference()], Some(&project.path))
            .expect("Stop the reviewed disposable guest.");
    }

    pub(in crate::projects) fn boot(&self, project: &Project) {
        let mut args = vec![
            "--json".into(),
            "--verbose".into(),
            "run".into(),
            "-d".into(),
        ];
        args.extend(
            self.brig
                .project_arguments(project, &self.settings)
                .unwrap(),
        );
        let output = crate::platform::command::run(
            &self.brig.executable,
            &args,
            Some(&project.path),
            &self.brig.environment,
            Duration::from_secs(180),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .expect("Start the reviewed disposable guest.");
        assert!(
            !output.timed_out,
            "The verified disposable guest did not boot within 180 seconds."
        );
        let boot = crate::platform::host::envelope(&output, "Run")
            .expect("Restart the reviewed disposable guest.");
        assert_eq!(boot["data"]["stage"], "detached");
        let sandbox = boot["data"]["sandbox"]
            .as_str()
            .expect("Brig did not name the verified guest.");
        if !output.stderr.contains("image and boot assets verified") {
            assert!(
                output.stderr.contains("already running") || output.stderr.contains("reuse"),
                "Brig did not report verification for this cold boot."
            );
        }
        self.brig
            .verify_running_image(project, sandbox)
            .expect("Confirm the signed running image digest.");
    }

    pub(in crate::projects) fn attach_policy(&self, project: &Project) {
        let args = [
            "policy",
            "attach",
            PROOF_POLICY,
            &project.profile,
            "-n",
            &project.label,
        ]
        .map(str::to_owned);
        self.brig
            .checked(&args, Some(&project.path))
            .expect("Attach the private proof policy.");
    }

    pub(in crate::projects) fn inventory(&self) -> serde_json::Value {
        self.brig
            .json(&["--json".into(), "ls".into()], "SandboxList", None)
            .unwrap()
    }
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Publication<'a>(&'a Proof, &'a Project);
impl Drop for Publication<'_> {
    fn drop(&mut self) {
        if self.0.dispatch(self.1, Action::Close, None).is_err() {
            eprintln!("Review the disposable preview before reusing this proof directory.");
        }
    }
}

pub(in crate::projects) fn register(registry: &mut Registry, root: &Path, label: &str) -> Project {
    let path = root.join(format!("project-{label}"));
    assert!(path.is_dir() && path.canonicalize().unwrap() == path);
    registry.register(&path).unwrap()
}

fn read_response(mapping: Mapping) -> String {
    let timeout = Duration::from_secs(3);
    let mut stream = TcpStream::connect_timeout(&mapping.address().into(), timeout).unwrap();
    stream.set_read_timeout(Some(timeout)).unwrap();
    stream.set_write_timeout(Some(timeout)).unwrap();
    write!(
        stream,
        "GET / HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        mapping.address()
    )
    .unwrap();
    let mut response = String::new();
    stream.take(4096).read_to_string(&mut response).unwrap();
    response.split_once("\r\n\r\n").unwrap().1.to_owned()
}

pub(in crate::projects) fn image_update_witness(
    checked: &environment::CheckedProject,
    file: &Path,
    action: Action,
    mapping: Mapping,
) -> Result<Approval, String> {
    let proof = Proof {
        brig: checked.brig.clone(),
        settings: checked.settings.clone(),
        file: file.to_owned(),
    };
    let project = &checked.project;
    let view = proof.dispatch(project, action, Some(mapping))?;
    if view.mapping != Some(mapping) || view.status != Status::WaitingForServer {
        return Err(format!(
            "The approved preview is not waiting for its server ({:?}).",
            view.status
        ));
    }
    let _server = proof.server(project, mapping.guest_port);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !runtime::server_responds(mapping) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    let view = proof.dispatch(project, Action::Read, None)?;
    if view.mapping != Some(mapping)
        || view.status != Status::Ready
        || read_response(mapping) != RECEIPT
    {
        return Err("The approved local preview did not serve the test response.".into());
    }
    Registry::load(file)?
        .record(&project.id)?
        .preview
        .clone()
        .ok_or_else(|| "The preview approval is missing.".into())
}

#[test]
#[ignore = "requires an isolated cached Hull proof store and local loopback access"]
fn publishes_and_closes_a_real_disposable_project_preview() {
    let root = PathBuf::from(
        env::var_os("TUGBOAT_PREVIEW_PROOF_ROOT").expect("Set the private proof directory."),
    );
    let records = Fixture::new();
    let proof = Proof::new(&root, records.file());
    let mut registry = Registry::default();
    let first = register(&mut registry, &root, "a");
    let sibling = register(&mut registry, &root, "b");
    crate::environment::managed_profile::install(&root.join("profiles")).unwrap();
    for project in [&first, &sibling] {
        println!(
            "Proof target {} uses {}.",
            project.reference(),
            proof.brig.shared_project_home(project).unwrap().display()
        );
    }
    let list = proof.inventory();
    let rows = list["data"]["sandboxes"].as_array().unwrap();
    assert!(
        rows.iter()
            .all(|row| row["ref"] == first.reference() || row["ref"] == sibling.reference()),
        "Use a private proof store with only the registered guests."
    );
    if env::var("TUGBOAT_PREVIEW_PROOF_BOOT").as_deref() == Ok("1") {
        for project in [&first, &sibling] {
            proof.attach_policy(project);
            proof.boot(project);
        }
    }
    let list = proof.inventory();
    let rows = list["data"]["sandboxes"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for project in [&first, &sibling] {
        let sandbox = format!("brig-{}-{}", project.profile, project.label);
        let home = proof.brig.shared_project_home(project).unwrap();
        assert!(rows.iter().any(|row| row["state"] == "running"
            && row["ref"] == project.reference()
            && row["sandbox"] == sandbox
            && row["workspace"].as_str() == home.to_str()));
    }
    storage::write(&proof.file, &registry).unwrap();
    assert_eq!(
        proof.dispatch(&first, Action::Read, None).unwrap().status,
        Status::Closed
    );
    assert_eq!(
        proof.dispatch(&sibling, Action::Read, None).unwrap().status,
        Status::Closed
    );
    let before = proof.boot_id(&first);
    let sibling_before = proof.boot_id(&sibling);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let mapping = mapping(listener.local_addr().unwrap().port());
    drop(listener);
    let _publication = Publication(&proof, &first);
    assert_eq!(
        proof
            .dispatch(&first, Action::Publish, Some(mapping))
            .unwrap()
            .status,
        Status::WaitingForServer
    );
    let _server = proof.server(&first, mapping.guest_port);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !runtime::server_responds(mapping) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        proof.dispatch(&first, Action::Read, None).unwrap().status,
        Status::Ready
    );
    assert_eq!(read_response(mapping), RECEIPT);
    let [project, claude] = Registry::load(&proof.file)
        .unwrap()
        .agent_views(&first.id)
        .unwrap();
    assert_eq!(project.preview, claude.preview);
    assert_eq!(project.reference(), claude.reference());
    {
        let _operation =
            environment::EnvironmentOperation::begin(&project.id, &project.environment_id).unwrap();
        open_verified(
            &proof.brig,
            &project,
            &proof.settings,
            mapping,
            runtime::server_responds,
            |approved| {
                assert_eq!(approved, mapping);
                println!("Verified {} serves proof project A.", approved.url());
                if env::var("TUGBOAT_PREVIEW_PROOF_OPEN").as_deref() == Ok("1") {
                    runtime::open(approved)?;
                    thread::sleep(Duration::from_secs(10));
                }
                Ok(())
            },
        )
        .unwrap();
    }
    assert_eq!(
        proof.dispatch(&first, Action::Close, None).unwrap().status,
        Status::Closed
    );
    assert!(
        TcpStream::connect_timeout(&mapping.address().into(), Duration::from_millis(600)).is_err()
    );
    proof.stop(&first);
    proof.boot(&first);
    assert_ne!(proof.boot_id(&first), before);
    assert_eq!(proof.boot_id(&sibling), sibling_before);
    assert_eq!(
        proof.dispatch(&first, Action::Read, None).unwrap().status,
        Status::Closed
    );
    assert_eq!(
        proof.dispatch(&sibling, Action::Read, None).unwrap().status,
        Status::Closed
    );
    assert!(
        TcpStream::connect_timeout(&mapping.address().into(), Duration::from_millis(600)).is_err()
    );
    println!("Closed the approved port and checked both projects after restarting A.");
}
