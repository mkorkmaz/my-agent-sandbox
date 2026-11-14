import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

test('legacy project login is rejected at the command boundary before project access or terminal reservation', () => {
  const source = fs.readFileSync(new URL('../../src-tauri/src/agent_sessions/mod.rs', import.meta.url), 'utf8');
  const command = source.match(/#\[tauri::command\]\npub async fn start_agent_session\([\s\S]*?\n}\n/);
  assert.ok(command, 'The native start_agent_session command must exist.');
  const body = command[0].split(') -> Result<u64, String> {')[1];
  assert.ok(body, 'The native launch result contract must remain available.');
  const rejection = body.match(/^\s*if action == Action::Login \{\s*return Err\("Connect the provider in Settings, Providers\. Sign-in does not require a project\."\.into\(\)\);\s*}/);
  assert.ok(rejection, 'Reject the legacy login action as the first command operation with the existing response.');
  for (const operation of ['window.app_handle()', 'selected(&app', 'terminal::reserve_identifier()']) {
    assert.ok(body.indexOf(operation) >= rejection[0].length, operation + ' must follow the legacy login rejection.');
  }
  assert.equal(body.match(/action == Action::Login/g).length, 1);
});
