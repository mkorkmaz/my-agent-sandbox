use super::*;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::utils::config::AppDirectoriesOverride;

const MANIFEST: &str = "/latest.json";
const ARCHIVE: &str = "/0.4.2/Tugboat.app.tar.gz";
const REPLACEMENT: &str = "/0.4.2-rebuilt/Tugboat.app.tar.gz";
const WAIT: Duration = Duration::from_secs(5);

#[test]
fn only_current_consent_can_request_an_archive() {
    let feed = ControlledFeed::new();
    let directory = ScheduleDirectory::new();
    let disabled = feed.app(&directory.0, "sh.brig.desktop.dev");
    assert_eq!(
        snapshot(check(disabled.handle(), false))["phase"],
        "disabled"
    );
    assert_eq!(
        snapshot(check(disabled.handle(), true))["phase"],
        "disabled"
    );
    assert!(!schedule_file(disabled.handle()).unwrap().exists());
    assert!(feed.requests().is_empty());

    let app = feed.app(&directory.0, "sh.brig.desktop");
    let app = app.handle();
    feed.release(ARCHIVE, "Original notes");
    let automatic = snapshot(check(app, false));
    assert_eq!(automatic["phase"], "available");
    assert_eq!(automatic["automaticOffer"], true);
    assert_eq!(feed.requests(), [MANIFEST]);

    let deferred = snapshot(later(app, offer_id(&automatic)).unwrap());
    assert_eq!(deferred["phase"], "available");
    assert_eq!(deferred["automaticOffer"], false);
    assert_eq!(snapshot(check(app, false))["automaticOffer"], false);
    assert_eq!(feed.requests(), [MANIFEST]);

    let manual = snapshot(check(app, true));
    assert_eq!(manual["phase"], "available");
    assert_eq!(manual["automaticOffer"], false);
    assert_eq!(feed.requests(), [MANIFEST, MANIFEST]);

    feed.release(ARCHIVE, " \t\n");
    let invalid = snapshot(check(app, true));
    assert_eq!(invalid["phase"], "failed");
    assert!(invalid["offer"].is_null());
    assert_eq!(approve(app, &manual)["phase"], "failed");
    assert_eq!(feed.requests(), [MANIFEST, MANIFEST, MANIFEST]);
    let restarted = feed.app(&directory.0, "sh.brig.desktop");
    assert_eq!(snapshot(check(restarted.handle(), false))["phase"], "idle");

    feed.release(ARCHIVE, "Original notes");
    let offer = snapshot(check(app, true));
    assert_eq!(offer["phase"], "available");
    assert_eq!(approve(app, &offer)["phase"], "downloading");
    wait_until(|| feed.requests().contains(&ARCHIVE.to_owned()));
    assert_eq!(approve(app, &offer)["phase"], "downloading");
    feed.finish_archive.send(()).unwrap();
    assert_signature_failure(app);
    assert_eq!(approve(app, &offer)["phase"], "failed");
    assert_eq!(
        feed.requests(),
        [MANIFEST, MANIFEST, MANIFEST, MANIFEST, ARCHIVE]
    );
    let restarted = feed.app(&directory.0, "sh.brig.desktop");
    assert_eq!(snapshot(check(restarted.handle(), false))["phase"], "idle");

    feed.release(REPLACEMENT, "Changed notes");
    let renewed = snapshot(check(app, true));
    assert_eq!(renewed["phase"], "available");
    assert_eq!(renewed["offer"]["version"], offer["offer"]["version"]);
    assert_eq!(renewed["offer"]["notes"], "Changed notes");
    assert_ne!(offer_id(&renewed), offer_id(&offer));
    assert_eq!(approve(app, &offer)["phase"], "available");
    assert_eq!(
        feed.requests(),
        [MANIFEST, MANIFEST, MANIFEST, MANIFEST, ARCHIVE, MANIFEST]
    );
    assert_eq!(approve(app, &renewed)["phase"], "downloading");
    feed.finish_archive.send(()).unwrap();
    assert_signature_failure(app);
    assert_eq!(
        feed.requests(),
        [
            MANIFEST,
            MANIFEST,
            MANIFEST,
            MANIFEST,
            ARCHIVE,
            MANIFEST,
            REPLACEMENT
        ]
    );
}

fn snapshot(status: Status) -> Value {
    serde_json::to_value(status).unwrap()
}

fn offer_id(status: &Value) -> u64 {
    status["offer"]["id"].as_u64().unwrap()
}

fn approve(app: &AppHandle<MockRuntime>, status: &Value) -> Value {
    let approval = consent::approve(
        app,
        offer_id(status),
        status["offer"]["sessionRevision"].as_u64().unwrap(),
        || Ok(()),
        |_, _, _| panic!("An invalid archive must never reach installation"),
    )
    .unwrap();
    snapshot(approval)
}

fn assert_signature_failure(app: &AppHandle<MockRuntime>) {
    let status = || snapshot(current_status(app, &mut State::lock(app)));
    wait_until(|| status()["phase"] == "failed");
    let failure = status();
    assert_eq!(
        failure["message"],
        "The update signature could not be verified."
    );
    assert!(failure["offer"].is_null());
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for the updater"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

struct ScheduleDirectory(PathBuf);

impl ScheduleDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tugboat-consent-test-{}-{}",
            std::process::id(),
            std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}

impl Drop for ScheduleDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct FeedState {
    manifest: String,
    requests: Vec<String>,
}

struct ControlledFeed {
    origin: String,
    state: Arc<Mutex<FeedState>>,
    finish_archive: Sender<()>,
    stop: Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl ControlledFeed {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let state = Arc::new(Mutex::new(FeedState::default()));
        let server_state = Arc::clone(&state);
        let (finish_archive, release_archive) = mpsc::channel();
        let (stop, running) = mpsc::channel();
        let worker = thread::spawn(move || {
            while let Err(RecvTimeoutError::Timeout) =
                running.recv_timeout(Duration::from_millis(5))
            {
                match listener.accept() {
                    Ok((stream, _)) => respond(stream, &server_state, || {
                        let _ = release_archive.recv_timeout(WAIT);
                    }),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(error) => panic!("Could not accept test request: {error}"),
                }
            }
        });
        Self {
            origin,
            state,
            finish_archive,
            stop,
            worker: Some(worker),
        }
    }

    fn app(&self, directory: &Path, identifier: &str) -> tauri::App<MockRuntime> {
        let mut context = mock_context(noop_assets());
        let config = context.config_mut();
        config.identifier = identifier.into();
        config.app.app_directories_override = Some(AppDirectoriesOverride::Root(directory.into()));
        config.plugins.0.insert(
            "updater".into(),
            json!({
                "pubkey": "fixture-key",
                "endpoints": [format!("{}{MANIFEST}", self.origin)],
                "dangerousInsecureTransportProtocol": true,
            }),
        );
        let app = mock_builder().build(context).unwrap();
        configure_identity(app.handle(), false).unwrap();
        app
    }

    fn release(&self, path: &str, notes: &str) {
        self.state.lock().unwrap().manifest = json!({
            "version": "0.4.2",
            "notes": notes,
            "platforms": { (TARGET): {
                "url": format!("{}{path}", self.origin),
                "signature": "invalid-fixture-signature",
            }},
        })
        .to_string();
    }

    fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }
}

impl Drop for ControlledFeed {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        let _ = self.finish_archive.send(());
        let _ = self.worker.take().unwrap().join();
    }
}

fn respond(mut stream: TcpStream, state: &Mutex<FeedState>, wait_for_approval: impl FnOnce()) {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.set_write_timeout(Some(WAIT)).unwrap();
    let mut request = String::new();
    let mut reader = BufReader::new(&mut stream);
    reader.read_line(&mut request).unwrap();
    let path = request.split_whitespace().nth(1).unwrap().to_owned();
    while request != "\r\n" {
        request.clear();
        if reader.read_line(&mut request).unwrap() == 0 {
            return;
        }
    }
    let manifest = {
        let mut state = state.lock().unwrap();
        state.requests.push(path.clone());
        state.manifest.clone()
    };
    let body = if path == MANIFEST {
        manifest.as_str()
    } else {
        wait_for_approval();
        "invalid archive"
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
