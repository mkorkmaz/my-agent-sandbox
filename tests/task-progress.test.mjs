import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

test('accepting a waiting task does not charge later work to that task', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'brig-progress-test-'));
  try {
    fs.mkdirSync(root + '/scripts'); fs.mkdirSync(root + '/artifacts/bd-03', { recursive: true });
    for (const file of ['task-progress.mjs', 'record-task-usage.mjs']) fs.copyFileSync(fileURLToPath(new URL('../scripts/' + file, import.meta.url)), root + '/scripts/' + file);
    const counter = input => ({ input_tokens: input, cached_input_tokens: 0, cache_write_input_tokens: 0, output_tokens: 0, total_tokens: input });
    const first = '2026-11-10T10:01:00Z', later = '2026-11-10T10:02:00Z';
    const measurement = { schemaVersion: 1, taskId: 'BD-03', turnId: 'implementation', model: 'gpt-6.1-sol', startedAt: '2026-11-10T10:00:00Z', baseline: { usage: counter(0) }, usageEndedAt: first, status: 'waiting', calls: [] };
    fs.writeFileSync(root + '/artifacts/bd-03/measurement.json', JSON.stringify(measurement));
    fs.writeFileSync(root + '/imp-plan.html', '<script type="application/json" id="agent-ledger">' + JSON.stringify({ items: { 'BD-03': { state: 'verification', log: [], periods: [{ from: measurement.startedAt, to: first }] } } }) + '</script>');
    const rows = [
      { type: 'event_msg', payload: { type: 'task_started', turn_id: measurement.turnId } },
      { type: 'turn_context', payload: { turn_id: measurement.turnId, model: measurement.model } },
      ...[[first, 100], [later, 10000]].map(([timestamp, input]) => ({ type: 'event_msg', timestamp, payload: { type: 'token_count', info: { total_token_usage: counter(input), last_token_usage: counter(input) } } })),
    ];
    const session = root + '/session.jsonl'; fs.writeFileSync(session, rows.map(row => JSON.stringify(row)).join('\n') + '\n');
    execFileSync(process.execPath, [root + '/scripts/task-progress.mjs', session, 'BD-03', 'done', 'Acceptance passed.', 'proof.json']);
    const accepted = JSON.parse(fs.readFileSync(root + '/artifacts/bd-03/measurement.json'));
    assert.equal(accepted.status, 'done');
    assert.equal(accepted.usageEndedAt, first);
    assert.equal(accepted.calls.length, 1);
    assert.equal(accepted.calls[0].input, 100);
    accepted.unmeasuredContributors = [{ role: 'finish reviewer', reason: 'No per-agent counter is available.' }];
    fs.writeFileSync(root + '/artifacts/bd-03/measurement.json', JSON.stringify(accepted));
    execFileSync(process.execPath, [root + '/scripts/task-progress.mjs', session, 'BD-03', 'log', 'Contributor usage is unavailable.']);
    const partial = JSON.parse(fs.readFileSync(root + '/artifacts/bd-03/measurement.json'));
    const html = fs.readFileSync(root + '/imp-plan.html', 'utf8');
    const ledger = JSON.parse(html.match(/<script[^>]*>([\s\S]*)<\/script>/)[1]);
    assert.equal(partial.calls[0].input, 100);
    assert.equal(partial.accounting.contributors[0].complete, false);
    assert.equal(ledger.items['BD-03'].coverage, 'partial');
  } finally { fs.rmSync(root, { recursive: true }); }
});
