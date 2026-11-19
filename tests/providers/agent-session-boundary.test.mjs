import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

test('launch commands check a complete target and sign-in belongs to the provider commands', () => {
  const source = fs.readFileSync(new URL('../../src-tauri/src/agent_sessions/mod.rs', import.meta.url), 'utf8');
  const command = source.match(/#\[tauri::command\]\npub async fn start_agent_session\([\s\S]*?\n}\n/);
  assert.ok(command, 'The native start_agent_session command must exist.');
  assert.match(command[0], /environment_id: EnvironmentId,\s+agent_kind: AgentKind,/);
  const selection = 'selected(&app, &identifier, &environment_id, agent_kind)?';
  assert.ok(command[0].includes(selection));
  for (const operation of ['codex::start_codex_session(', 'terminal::reserve_identifier()']) {
    assert.ok(command[0].indexOf(operation) > command[0].indexOf(selection), operation + ' must follow target validation.');
  }
  const entryPoint = fs.readFileSync(new URL('../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  assert.match(entryPoint, /provider_login::start_provider_login,/);
  assert.match(entryPoint, /provider_login::cancel_provider_login,/);
});

test('the Codex adapter is available only through the checked agent command', () => {
  const source = fs.readFileSync(new URL('../../src-tauri/src/agent_sessions/codex.rs', import.meta.url), 'utf8');
  const command = source.match(/pub\(super\) async fn start_codex_session\([\s\S]*?\n}\n/);
  assert.ok(command, 'The Codex adapter must stay internal.');
  const entryPoint = fs.readFileSync(new URL('../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  assert.doesNotMatch(entryPoint, /codex::start_codex_session/);
  assert.match(command[0], /environment::checked_agent_environment\(\s*&app,\s*&project.id,\s*&project.environment_id,\s*AgentKind::Codex,/);
});
