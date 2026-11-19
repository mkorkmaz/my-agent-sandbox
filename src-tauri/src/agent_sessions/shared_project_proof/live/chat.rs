use super::{checked_plan, publish_access, require_provider_status, runner, Guest, REFERENCE};
use crate::agent_sessions::shared_project_proof::{explicit_arguments, Terminal};
use crate::agents::{self, AgentCommand, AgentKind, SessionId};
use serde_json::Value;
use std::{env, fs, path::Path, process::Command, time::Duration};

const RESPONSE: Duration = Duration::from_secs(90);

fn launch(guest: &Guest, project: &Path, kind: AgentKind, action: AgentCommand<'_>) -> Terminal {
    let resumed = matches!(action, AgentCommand::Resume(_));
    let command = match kind {
        AgentKind::Codex => agents::codex::command(action, agents::codex::Boundary::Brig),
        AgentKind::ClaudeCode => kind.command(action),
    }
    .unwrap();
    let mut host = Command::new(&guest.executable);
    host.env_clear()
        .envs(&guest.environment)
        .current_dir(project)
        .args(explicit_arguments(kind, command));
    let mut terminal = Terminal::launch(&mut host);
    match kind {
        AgentKind::Codex => {
            terminal.receive("OpenAI Codex");
        }
        AgentKind::ClaudeCode => {
            let ready = if resumed {
                &["Claude Code", "Choose the text style", "❯"][..]
            } else {
                &["Claude Code", "Choose the text style"][..]
            };
            let screen = terminal.wait_for(ready, Duration::from_secs(30));
            if screen.contains("Choose the text style") {
                terminal.send("\r");
                terminal.receive("Claude Code");
            }
        }
    }
    terminal
}

fn send_prompt(terminal: &mut Terminal, prompt: &str) {
    terminal.send(&format!("\x1b[200~{prompt}\x1b[201~"));
    std::thread::sleep(Duration::from_millis(300));
    terminal.send("\r");
}

fn codex_session(directory: &Path, marker: &str, depth: u8) -> Option<SessionId> {
    for entry in fs::read_dir(directory).ok()?.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth > 0 {
            if let Some(session) = codex_session(&entry.path(), marker, depth - 1) {
                return Some(session);
            }
        }
        if !kind.is_file()
            || entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        if !entry
            .metadata()
            .is_ok_and(|metadata| metadata.len() <= 1_048_576)
        {
            continue;
        }
        let Ok(transcript) = fs::read_to_string(entry.path()) else {
            continue;
        };
        if !transcript.contains(marker) {
            continue;
        }
        let first: Value = serde_json::from_str(transcript.lines().next()?).ok()?;
        if first["type"] == "session_meta" {
            return SessionId::new(first["payload"]["id"].as_str()?).ok();
        }
    }
    None
}

fn boot_id(guest: &Guest) -> String {
    let result = guest.run(
        &[
            "exec",
            REFERENCE,
            "--",
            "cat",
            "/proc/sys/kernel/random/boot_id",
        ],
        None,
    );
    assert!(result.exit_code == Some(0));
    let id = result.stdout.trim();
    SessionId::new(id).expect("The guest returned an invalid boot ID");
    id.into()
}

fn tool_requests(transcript: &Path) -> Vec<Value> {
    let Ok(transcript) = fs::read_to_string(transcript) else {
        return Vec::new();
    };
    assert!(transcript.len() < 1_048_576);
    transcript
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["type"] == "assistant")
        .filter_map(|event| event["message"]["content"].as_array().cloned())
        .flatten()
        .filter(|item| item["type"] == "tool_use")
        .collect()
}

fn requested_command(item: &Value, expected: &str) -> bool {
    item["name"] == "Bash" && item["input"]["command"] == expected
}

fn claude_reply_after_prompt(bytes: &[u8], session: &str, prompt: &str, expected: &str) -> bool {
    let mut prompted = false;
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(event) = serde_json::from_slice::<Value>(line) else { continue; };
        if event["sessionId"] != session { continue; }
        let message = &event["message"];
        let content = &message["content"];
        let text = |expected| content == expected || content.as_array().is_some_and(|parts| {
            parts.iter().any(|part| part["type"] == "text" && part["text"] == expected)
        });
        if event["type"] == "user" && text(prompt) { prompted = true; }
        if prompted && event["type"] == "assistant" && message["model"].as_str().is_some_and(|model| !model.is_empty() && model != "<synthetic>") && text(expected) {
            return true;
        }
    }
    false
}

fn require_resumed_reply(terminal: &mut Terminal, kind: AgentKind, transcript: &Path, session: &SessionId, prompt: &str, expected: &str) {
    let offset = if kind == AgentKind::ClaudeCode { fs::metadata(transcript).unwrap().len() as usize } else { 0 };
    send_prompt(terminal, prompt);
    if kind == AgentKind::Codex {
        terminal.wait_for(&[expected], RESPONSE);
        return;
    }
    let session = session.argument();
    terminal.wait_until(RESPONSE, "the new provider reply in the resumed Claude transcript", |_| {
        crate::storage::read_regular_bytes(transcript).is_ok_and(|bytes| bytes.is_some_and(|bytes| {
            bytes.get(offset..).is_some_and(|new| claude_reply_after_prompt(new, &session, prompt, expected))
        }))
    });
}

#[test]
fn resumed_reply_needs_a_new_prompt_and_a_real_reply_in_the_same_session() {
    let response = br#"{"type":"user","sessionId":"proof","message":{"content":"recall"}}
{"type":"assistant","sessionId":"proof","message":{"model":"<synthetic>","content":[{"type":"text","text":"known marker"}]}}
{"type":"assistant","sessionId":"other","message":{"model":"real","content":[{"type":"text","text":"known marker"}]}}"#;
    assert!(!claude_reply_after_prompt(response, "proof", "recall", "known marker"));
    let reply = br#"{"type":"assistant","sessionId":"proof","message":{"model":"real","content":[{"type":"text","text":"known marker"}]}}"#;
    assert!(!claude_reply_after_prompt(reply, "proof", "recall", "known marker"));
    assert!(claude_reply_after_prompt(&[response.as_slice(), b"\n", reply.as_slice()].concat(), "proof", "recall", "known marker"));
}

fn one_time_bash_approval(screen: &str, source: &str, target: &str) -> bool {
    screen.contains("Bash command")
        && screen.contains(source)
        && screen.contains(target)
        && screen.lines().any(|line| {
            line.chars()
                .filter(|character| !character.is_whitespace())
                .eq("❯1.Yes".chars())
        })
}

#[test]
fn bash_requires_a_displayed_one_time_approval_for_the_fixture() {
    let screen =
        "Bash command\ncat source.txt > target.txt\n❯ 1. Yes\n  2. Yes, and always allow\n";
    assert!(one_time_bash_approval(screen, "source.txt", "target.txt"));
    assert!(!one_time_bash_approval(
        screen,
        "source.txt",
        "different.txt"
    ));
    assert!(!one_time_bash_approval(
        &screen.replace("❯ 1. Yes", "❯ 1. Yes, and always allow"),
        "source.txt",
        "target.txt"
    ));
}

fn rendered_controls(output: &str) -> String {
    let mut rendered = String::new();
    let mut stream = output.chars();
    while let Some(character) = stream.next() {
        if character != '\x1b' {
            if !character.is_whitespace() {
                rendered.push(character);
            }
            continue;
        }
        match stream.next() {
            Some('[') => {
                stream.by_ref().find(|value| ('@'..='~').contains(value));
            }
            Some(']') => {
                stream.by_ref().find(|value| matches!(value, '\x07' | '\\'));
            }
            _ => {}
        }
    }
    rendered
}

fn selected_conversation_restore(controls: &str) -> bool {
    controls.contains("❯1.Restoreconversation") || controls.contains("❯2.Restoreconversation")
}

#[test]
fn restore_requires_the_selected_conversation_only_control() {
    let conversation =
        "\x1b[38;5;153m❯\x1b[5G\x1b[38;5;246m1. \x1b[38;5;153mRestore conversation\r";
    assert!(selected_conversation_restore(&rendered_controls(
        conversation
    )));
    let code = "❯ 1. Restore code and conversation\r  2. Restore conversation\r";
    assert!(!selected_conversation_restore(&rendered_controls(code)));
}

fn rewind_conversation(terminal: &mut Terminal) {
    std::thread::sleep(Duration::from_millis(900));
    terminal.send("/rewind\r");
    terminal.receive("(current)");
    terminal.send("\x1b[H");
    std::thread::sleep(Duration::from_millis(750));
    terminal.send("\r");
    let screen = terminal.wait_until(
        Duration::from_secs(10),
        "the Claude conversation restore choices",
        |output| {
            let controls = rendered_controls(output);
            controls.contains("Restoreconversation") && controls.contains("Summarizefromhere")
        },
    );
    let confirmation = rendered_controls(&screen);
    if !selected_conversation_restore(&confirmation) {
        assert!(
            confirmation.contains("❯1.Restorecodeandconversation")
                && confirmation.contains("2.Restoreconversation"),
            "Could not identify the conversation-only action in the recovery menu"
        );
        terminal.send("\x1b[B");
        terminal.wait_until(
            Duration::from_secs(5),
            "the selected conversation-only restore action",
            |output| selected_conversation_restore(&rendered_controls(output)),
        );
    }
    std::thread::sleep(Duration::from_millis(750));
    terminal.send("\r");
    std::thread::sleep(Duration::from_millis(900));
    println!("Restored only the Claude fixture conversation to its first checkpoint");
}

fn restart(guest: &Guest, root: &Path, guest_home: &Path, original_boot: &str) {
    assert!(
        guest.run(&["stop", REFERENCE], None).exit_code == Some(0),
        "Could not stop the disposable guest"
    );
    let project = root.join("sample project");
    let restarted = guest.run(
        &[
            "--verbose",
            "run",
            "-d",
            "--network",
            "isolated",
            REFERENCE,
            project.to_str().unwrap(),
        ],
        None,
    );
    assert!(
        restarted.exit_code == Some(0)
            && restarted.stderr.contains("image and boot assets verified"),
        "Could not restart the verified guest"
    );
    checked_plan(root, guest);
    assert_ne!(boot_id(guest), original_boot);
    let cleared = guest.run(
        &[
            "exec",
            REFERENCE,
            "--",
            "sh",
            "-ec",
            "test ! -e \"$CLAUDE_CONFIG_DIR/.credentials.json\"",
        ],
        None,
    );
    assert!(
        cleared.exit_code == Some(0),
        "The Claude access copy survived guest stop"
    );
    assert!(!guest_home.join(".claude/.credentials.json").exists());
    println!("The verified guest restarted and cleared its Claude access copy");

    let (_, claude_access) = super::current_access();
    assert!(guest.credentials("write", Some(serde_json::to_vec(&claude_access).unwrap())));
    require_provider_status(guest);
}

#[test]
#[ignore = "uses the authorized live Tugboat Dev provider connections and restarts the disposable guest"]
fn both_live_chats_use_project_files_and_resume_after_stop() {
    let runner = runner();
    let root = runner.parent().unwrap();
    let guest = Guest::from_root(root);
    let guest_home = checked_plan(root, &guest);
    let project = root.join("sample project");
    let copies = publish_access(&guest, &guest_home);
    require_provider_status(&guest);
    let original_boot = boot_id(&guest);
    let codex_config = guest_home.join(".codex/config.toml");
    crate::storage::write_bytes(
        &codex_config,
        b"[projects.\"/work/sample project\"]\ntrust_level = \"trusted\"\n",
    )
    .unwrap();

    let continue_id = env::var("TUGBOAT_PHASE1_CONTINUE_ID").ok();
    let claude_id = match continue_id.as_deref() {
        Some(id) => SessionId::new(id),
        None => SessionId::generate(),
    }
    .unwrap();
    let label = claude_id.argument();
    let codex_marker = format!("codex-{}", &label[..8]);
    let claude_marker = format!("claude-{}", &label[..8]);
    let codex_file = format!("codex-proof-{}.txt", &label[..8]);
    let claude_file = format!("claude-proof-{}.txt", &label[..8]);
    let previous_codex = codex_session(&guest_home.join(".codex/sessions"), &codex_marker, 4);
    assert_eq!(
        previous_codex.is_some(),
        continue_id.is_some(),
        "The continued Codex session did not match the fixture"
    );
    let mut codex = launch(
        &guest,
        &project,
        AgentKind::Codex,
        previous_codex
            .as_ref()
            .map_or(AgentCommand::NewChat(None), AgentCommand::Resume),
    );
    let codex_path = project.join(&codex_file);
    if continue_id.is_none() {
        send_prompt(&mut codex, &format!("Use the shell to write {codex_marker} to {codex_file} in the current directory with no newline. Reply CODEX-PROOF-DONE when the file exists."));
        codex.wait_until(RESPONSE, "Codex to write the project fixture", |_| {
            fs::read_to_string(&codex_path).is_ok_and(|text| text == codex_marker)
        });
    }
    assert_eq!(fs::read_to_string(&codex_path).unwrap(), codex_marker);
    println!("Codex project fixture {codex_file} has the expected contents");

    let claude_transcript = guest_home
        .join(".claude/projects/-work-sample-project")
        .join(format!("{label}.jsonl"));
    assert_eq!(claude_transcript.is_file(), continue_id.is_some());
    let mut claude = launch(
        &guest,
        &project,
        AgentKind::ClaudeCode,
        if continue_id.is_some() {
            AgentCommand::Resume(&claude_id)
        } else {
            AgentCommand::NewChat(Some(&claude_id))
        },
    );
    let shell = format!("cat {codex_file} && printf '%s' '{claude_marker}' > {claude_file}");
    let prior_requests = tool_requests(&claude_transcript);
    if continue_id.is_some() && env::var("TUGBOAT_PHASE1_REWIND").as_deref() == Ok("1") {
        rewind_conversation(&mut claude);
        claude.send("\r");
    } else {
        send_prompt(&mut claude, &format!("Make one Bash tool call to run this exact command now, then reply with its stdout: {shell}"));
    }
    let claude_path = project.join(&claude_file);
    claude.wait_for_screen(
        RESPONSE,
        "the Claude manual permission prompt for the fixture command",
        |screen| {
            let requests = tool_requests(&claude_transcript);
            let mut new = requests
                .iter()
                .filter(|request| !prior_requests.iter().any(|old| old["id"] == request["id"]));
            new.next()
                .is_some_and(|request| requested_command(request, &shell))
                && new.next().is_none()
                && one_time_bash_approval(screen, &codex_file, &claude_file)
        },
    );
    assert!(
        !claude_path.exists(),
        "Claude executed the command before approval"
    );
    println!("Claude requested manual approval for the exact fixture command");
    std::thread::sleep(Duration::from_millis(750));
    claude.send("\r");
    claude.wait_until(
        RESPONSE,
        "Claude to read and write the project fixtures",
        |output| {
            output.contains(&codex_marker)
                && fs::read_to_string(&claude_path).is_ok_and(|text| text == claude_marker)
        },
    );

    send_prompt(&mut codex, &format!("Read {claude_file} using the shell. Reply with the exact contents, with no other text."));
    codex.wait_for(&[&claude_marker], RESPONSE);
    let codex_id = codex_session(&guest_home.join(".codex/sessions"), &codex_marker, 4)
        .expect("Codex did not persist its native session ID");
    assert_eq!(boot_id(&guest), original_boot);
    drop(claude);
    drop(codex);

    restart(&guest, root, &guest_home, &original_boot);
    for (kind, native_id, original_marker) in [
        (AgentKind::Codex, codex_id, &claude_marker),
        (AgentKind::ClaudeCode, claude_id, &codex_marker),
    ] {
        let mut terminal = launch(&guest, &project, kind, AgentCommand::Resume(&native_id));
        require_resumed_reply(&mut terminal, kind, &claude_transcript, &native_id,
            "From this conversation, recall the exact contents you read from the other program's file. Do not use a tool. Output the word RESUMED, one space, then those contents.",
            &format!("RESUMED {original_marker}"));
        println!("Resumed {kind:?} session {}", native_id.argument());
    }
    assert!(
        copies.remove(),
        "Could not remove the disposable access copies"
    );
}

#[test]
#[ignore = "uses the two existing authorized test chats and restarts the disposable guest"]
fn recorded_live_chats_resume_after_stop() {
    let runner = runner();
    let root = runner.parent().unwrap();
    let guest = Guest::from_root(root);
    let guest_home = checked_plan(root, &guest);
    let project = root.join("sample project");
    let id = env::var("TUGBOAT_PHASE1_CONTINUE_ID")
        .expect("Select the previously created Claude proof chat");
    let claude_id = SessionId::new(&id).unwrap();
    let label = &id[..8];
    let codex_marker = format!("codex-{label}");
    let claude_marker = format!("claude-{label}");
    let codex_id = codex_session(&guest_home.join(".codex/sessions"), &codex_marker, 4)
        .expect("The matched Codex proof chat does not exist");
    let claude_transcript = guest_home
        .join(".claude/projects/-work-sample-project")
        .join(format!("{id}.jsonl"));
    let shell = format!(
        "cat codex-proof-{label}.txt && printf '%s' 'claude-{label}' > claude-proof-{label}.txt"
    );
    assert!(tool_requests(&claude_transcript)
        .iter()
        .any(|request| requested_command(request, &shell)));
    let copies = publish_access(&guest, &guest_home);
    let original_boot = boot_id(&guest);
    restart(&guest, root, &guest_home, &original_boot);
    for (kind, native_id, marker) in [
        (AgentKind::Codex, codex_id, codex_marker),
        (AgentKind::ClaudeCode, claude_id, claude_marker),
    ] {
        let mut terminal = launch(&guest, &project, kind, AgentCommand::Resume(&native_id));
        require_resumed_reply(&mut terminal, kind, &claude_transcript, &native_id,
            "From this conversation, recall the exact short test marker you were asked to write. Do not use a tool. Output the word RESUMED, one space, then that marker.",
            &format!("RESUMED {marker}"));
        println!("Resumed {kind:?} session {}", native_id.argument());
    }
    assert!(
        copies.remove(),
        "Could not remove the disposable access copies"
    );
}
