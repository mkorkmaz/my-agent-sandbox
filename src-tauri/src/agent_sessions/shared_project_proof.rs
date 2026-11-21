use crate::{
    agents::{self, AgentCommand, AgentKind, GuestCommand, SessionId},
    terminal::pty::{Process, Pty},
};
use std::{
    env, fs,
    io::{Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};

mod live;
mod screen;

const REFERENCE: &str = "tugboat-project-proof@proof";
const CODEX: &str = "/root/.local/share/tugboat-phase1/codex/bin/codex";

fn explicit_arguments(agent: AgentKind, command: GuestCommand) -> Vec<String> {
    let mut arguments = command.in_guest(REFERENCE.into());
    arguments[2] = match agent {
        AgentKind::Codex => CODEX,
        AgentKind::ClaudeCode => "claude",
    }
    .into();
    arguments
}

#[test]
fn shared_guest_launches_use_the_agent_adapter_and_explicit_executable() {
    let id = SessionId::new("2827b5ef-87a4-47e9-a020-1cc78d09c8f2").unwrap();
    for (boundary, expected) in [
        (agents::codex::Boundary::Brig, "danger-full-access"),
        (agents::codex::Boundary::Workspace, "workspace-write"),
    ] {
        let command = agents::codex::command(AgentCommand::Resume(&id), boundary).unwrap();
        let arguments = explicit_arguments(AgentKind::Codex, command);
        assert_eq!(
            &arguments[..6],
            [
                "sh",
                REFERENCE,
                CODEX,
                "--no-daemon",
                "resume",
                &id.argument()
            ]
        );
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["--sandbox", expected]));
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["--ask-for-approval", "never"]));
    }
    let command = AgentKind::ClaudeCode
        .command(AgentCommand::NewChat(Some(&id)))
        .unwrap();
    assert_eq!(
        explicit_arguments(AgentKind::ClaudeCode, command),
        [
            "sh",
            REFERENCE,
            "claude",
            "--session-id",
            &id.argument(),
            "--permission-mode",
            "manual"
        ]
    );
    let command = AgentKind::ClaudeCode
        .command(AgentCommand::Resume(&id))
        .unwrap();
    assert_eq!(
        explicit_arguments(AgentKind::ClaudeCode, command),
        [
            "sh",
            REFERENCE,
            "claude",
            "--resume",
            &id.argument(),
            "--permission-mode",
            "manual"
        ]
    );
}

struct Terminal {
    input: (fs::File, screen::Screen),
    child: (Process, Child),
}

impl Terminal {
    fn open(runner: &Path, arguments: &[&str]) -> Self {
        let mut command = Command::new(runner);
        command.env_clear().arg("brig").args(arguments);
        Self::launch(&mut command)
    }

    fn launch(command: &mut Command) -> Self {
        Self::launch_sized(command, screen::COLUMNS, screen::ROWS)
    }

    fn launch_sized(command: &mut Command, columns: u16, rows: u16) -> Self {
        let pty = Pty::open().unwrap();
        let process = Process::new(&pty).unwrap();
        process.resize(columns, rows, 8 * columns, 16 * rows);
        let child = process.spawn(&pty, command).unwrap();
        Self {
            input: (
                pty.master.try_clone().unwrap(),
                screen::Screen::new(pty.master.try_clone().unwrap(), columns, rows),
            ),
            child: (process, child),
        }
    }

    fn receive(&mut self, expected: &str) -> String {
        self.wait_for(&[expected], Duration::from_secs(30))
    }

    fn wait_for(&mut self, expected: &[&str], timeout: Duration) -> String {
        self.wait_until(timeout, &format!("{expected:?}"), |output| {
            expected.iter().any(|text| output.contains(text))
        })
    }

    fn wait_until(
        &mut self,
        timeout: Duration,
        expected: &str,
        check: impl Fn(&str) -> bool,
    ) -> String {
        self.wait_for_output(timeout, expected, false, check)
    }

    fn wait_for_screen(
        &mut self,
        timeout: Duration,
        expected: &str,
        check: impl Fn(&str) -> bool,
    ) -> String {
        self.wait_for_output(timeout, expected, true, check)
    }

    fn wait_for_output(
        &mut self,
        timeout: Duration,
        expected: &str,
        use_screen: bool,
        check: impl Fn(&str) -> bool,
    ) -> String {
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        let end = Instant::now() + timeout;
        let mut descriptor = libc::pollfd {
            fd: self.input.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        while Instant::now() < end {
            let screen = if use_screen {
                self.input.1.visible_text().into()
            } else {
                String::from_utf8_lossy(&output)
            };
            if check(&screen) {
                return screen.into_owned();
            }
            let ready = unsafe { libc::poll(&mut descriptor, 1, 250) };
            if ready <= 0 {
                continue;
            }
            let count = self
                .input
                .0
                .read(&mut buffer)
                .expect("Brig terminal closed before the expected response");
            assert!(
                count > 0 && output.len() < 1_048_576,
                "Bad terminal response"
            );
            output.extend_from_slice(&buffer[..count]);
            self.input.1.feed(&buffer[..count]);
        }
        let output = self.input.1.visible_text();
        let observed = [
            "OpenAI Codex",
            "Claude Code",
            "Choose the text style",
            "API Error",
            "Do you trust",
            "Do you want to proceed",
            "rate limit",
            "not logged in",
            "Sign in",
            "Bash command",
            "Waiting for permission",
            "1. Yes",
            "Esc to cancel",
            "No response requested.",
        ];
        let present = observed
            .into_iter()
            .filter(|marker| output.contains(marker))
            .collect::<Vec<_>>();
        panic!("Brig terminal timed out waiting for {expected}; recognized parts: {present:?}");
    }

    fn send(&mut self, text: &str) {
        self.input.0.write_all(text.as_bytes()).unwrap();
    }
}

pub(crate) fn replay_without_message(
    checked: &crate::environment::CheckedProject,
    agent: AgentKind,
    id: &SessionId,
    marker: &str,
) -> Result<(), String> {
    if agent == AgentKind::Codex && env::var_os("TUGBOAT_IMAGE_TEST_CHATGPT_RECORDS").is_some() {
        return crate::providers::shared_chatgpt::with_access_copy(checked, || {
            let arguments = super::codex::proof_resume_arguments(checked, id)?;
            let display = observe_replay(checked, agent, id, arguments, marker);
            if matches!(display, Ok(Replay::HistoricalReply)) {
                return Ok(());
            }
            let native = native_codex_resume(checked, id, marker);
            match (display, native) {
                (Ok(Replay::ResolvedOnExit), Ok(())) => Ok(()),
                (Err(display), Ok(())) => Err(format!("{display} The local native protocol resumed the selected ID and returned its saved reply.")),
                (Err(display), Err(native)) => Err(format!("{display} {native}")),
                (_, Err(native)) => Err(native),
                (Ok(Replay::HistoricalReply), Ok(())) => Ok(()),
            }
        });
    }
    if agent == AgentKind::ClaudeCode {
        super::resume::require(checked, id)?;
    }
    let arguments = crate::environment::shared_runtime::interactive(
        &checked.project,
        agent.command(AgentCommand::Resume(id))?,
    );
    observe_replay(checked, agent, id, arguments, marker).map(|_| ())
}

#[derive(PartialEq)]
enum Replay {
    HistoricalReply,
    ResolvedOnExit,
}

fn observe_replay(
    checked: &crate::environment::CheckedProject,
    agent: AgentKind,
    id: &SessionId,
    arguments: Vec<String>,
    marker: &str,
) -> Result<Replay, String> {
    let mut terminal = Terminal::launch_sized(
        &mut crate::environment::command(checked, arguments),
        140,
        66,
    );
    let mut descriptor = libc::pollfd {
        fd: terminal.input.0.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let started = Instant::now();
    let end = started + Duration::from_secs(60);
    let mut buffer = [0; 4096];
    let mut output = Vec::new();
    let mut witnessed = std::collections::BTreeSet::new();
    let mut titles = crate::terminal::TitleParser::default();
    let expected_id = id.argument().to_ascii_lowercase();
    let continuation = format!("codex resume {expected_id}");
    let mut view_requested = false;
    let mut quit_requested = false;
    let mut composer_visible = false;
    let mut selected_ready = false;
    let mut ending = "deadline";
    let mut read_error = None;
    let states = [
        "OpenAI Codex",
        "Claude Code",
        "Sign in to ChatGPT",
        "Sign in with ChatGPT",
        "Select login method",
        "Choose the text style",
        "Claude account with subscription",
        "Not logged in",
        "Do you trust",
        "Trust this directory",
        "No saved session",
        "No conversation found",
        "No session found",
        "Could not resume",
        "To continue this session",
        "latest",
        "failed to load",
        "Reconnecting",
        "stream disconnected",
        "model list",
        "for shortcuts",
    ];
    while Instant::now() < end {
        if agent == AgentKind::Codex && composer_visible {
            if !view_requested && (selected_ready || started.elapsed() >= Duration::from_secs(45)) {
                // Ctrl+T opens the Codex read-only transcript.
                terminal
                    .input
                    .0
                    .write_all(b"\x14")
                    .map_err(|_| "Could not open the disposable Codex transcript view.")?;
                witnessed.insert("sent Codex transcript shortcut");
                view_requested = true;
            }
            if selected_ready && !quit_requested && started.elapsed() >= Duration::from_secs(55) {
                terminal.input.0.write_all(b"\x04").map_err(|_| {
                    "Could not close the empty Codex composer for the native resume hint."
                })?;
                witnessed.insert("requested graceful Codex exit");
                quit_requested = true;
            }
        }
        if unsafe { libc::poll(&mut descriptor, 1, 200) } <= 0 {
            continue;
        }
        let count = match terminal.input.0.read(&mut buffer) {
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue
            }
            Err(error) => {
                ending = "PTY read error";
                read_error = error.raw_os_error();
                break;
            }
        };
        if count == 0 {
            ending = "PTY reached EOF";
            break;
        }
        for title in titles.feed(&buffer[..count]) {
            if let Some(prefix) = title.prefix {
                selected_ready = expected_id.starts_with(&prefix) && !title.busy;
                witnessed.insert(if expected_id.starts_with(&prefix) {
                    "matching native session title"
                } else {
                    "other native session title"
                });
            }
        }
        terminal.input.1.feed(&buffer[..count]);
        let history = terminal.input.1.history_text();
        output.extend_from_slice(&buffer[..count]);
        if history.contains(marker)
            || output
                .windows(marker.len())
                .any(|part| part == marker.as_bytes())
        {
            return Ok(Replay::HistoricalReply);
        }
        if agent == AgentKind::Codex
            && quit_requested
            && history.contains("To continue this session")
            && history.contains(&continuation)
        {
            println!("The native Codex terminal exited with the exact same-session resume hint.");
            return Ok(Replay::ResolvedOnExit);
        }
        witnessed.extend(
            states
                .iter()
                .copied()
                .filter(|state| history.contains(*state)),
        );
        let visible = terminal.input.1.visible_text();
        composer_visible = visible.contains("OpenAI Codex") && visible.contains("for shortcuts");
        output.drain(..output.len().saturating_sub(192));
    }
    if ending != "deadline" {
        for _ in 0..5 {
            if terminal.child.1.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let status = terminal.child.1.try_wait().ok().flatten().map(|status| {
        (
            status.code(),
            std::os::unix::process::ExitStatusExt::signal(&status),
        )
    });
    Err(format!(
        "{} did not display its saved chat. Terminal states: {witnessed:?}. {ending}, error {read_error:?}, exit status {status:?}.", agent.binary(),
    ))
}

fn native_codex_resume(
    checked: &crate::environment::CheckedProject,
    id: &SessionId,
    marker: &str,
) -> Result<(), String> {
    use crate::{environment::shared_runtime, platform::command};
    let arguments = shared_runtime::exec(
        &checked.project,
        "python3",
        &[
            "-c",
            include_str!("shared_project_proof/codex_replay.py"),
            shared_runtime::program(AgentKind::Codex),
            &id.argument(),
            marker,
        ],
    );
    let output = command::run(
        &checked.brig.executable,
        &arguments,
        Some(&checked.project.path),
        &checked.brig.environment,
        Duration::from_secs(60),
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    let status = output.stdout.trim();
    if output.exit_code == Some(0)
        && !output.timed_out
        && !output.cancelled
        && !output.output_exceeded
    {
        if status == "exact-native-resume" {
            println!("Pinned Codex thread/resume returned the exact original ID and its latest saved reply.");
            return Ok(());
        }
        let known = [
            "initialize-closed",
            "initialize-refused",
            "initialize-timeout",
            "initialize-oversized",
            "resume-closed",
            "resume-refused",
            "resume-timeout",
            "resume-oversized",
            "wrong-thread",
            "missing-historical-assistant-reply",
            "unsupported-local-protocol",
        ];
        if known.contains(&status)
            || status
                .strip_prefix("resume-rpc-")
                .is_some_and(|code| code.parse::<i32>().is_ok())
        {
            return Err(format!("The pinned Codex local resume returned {status}."));
        }
    }
    Err("The pinned Codex local resume command did not return verifiable evidence.".into())
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.child.0.stop(libc::SIGKILL);
        let _ = self.child.1.wait();
        self.child.0.exited();
    }
}

fn runner() -> PathBuf {
    let root = fs::canonicalize(env::var_os("TUGBOAT_PHASE1_ROOT").unwrap()).unwrap();
    assert_eq!(root.parent(), Some(Path::new("/private/tmp")));
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("tgp1."));
    root.join("run")
}

#[test]
#[ignore = "requires a verified disposable Brig guest from the Phase 1 runner"]
fn two_guest_commands_keep_their_native_terminal_input_targets() {
    let runner = runner();
    let script = r#"test -t 0 && test -t 1 && test -t 2 && test "$PWD" = "$BRIG_PROJECT" || exit 1
        stty -echo
        "$1" --version || exit 1
        printf 'phase1-pty-ready\n'
        IFS= read -r line
        printf '%s:%s\n' "$2" "$line""#;
    let mut codex = Terminal::open(
        &runner,
        &[
            "sh", REFERENCE, "sh", "-c", script, "phase1", CODEX, "codex",
        ],
    );
    assert!(codex
        .receive("phase1-pty-ready")
        .contains("codex-cli 0.171.0"));
    let mut claude = Terminal::open(
        &runner,
        &[
            "sh", REFERENCE, "sh", "-c", script, "phase1", "claude", "claude",
        ],
    );
    assert!(claude.receive("phase1-pty-ready").contains("(Claude Code)"));
    codex.send("first-input\n");
    claude.send("second-input\n");
    codex.receive("codex:first-input");
    claude.receive("claude:second-input");
}
