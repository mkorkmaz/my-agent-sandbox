import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { recordUsage } from './record-task-usage.mjs';

const [sessionPath, taskId, action, note, evidence] = process.argv.slice(2);
assert.match(taskId ?? '', /^BD-\d{2}$/);
assert.ok(['start', 'wait', 'done', 'log'].includes(action) && note);
const root = fileURLToPath(new URL('../', import.meta.url));
const rows = fs.readFileSync(sessionPath, 'utf8').trim().split('\n').map(JSON.parse);
const latest = rows.filter(row => row.type === 'event_msg' && row.payload.type === 'token_count' && row.payload.info).at(-1);
const turn = rows.filter(row => row.type === 'turn_context').at(-1);
assert.equal(turn.payload.model, 'gpt-6.1-sol');
const now = new Date().toISOString();
const file = path.join(root, 'artifacts', taskId.toLowerCase(), 'measurement.json');
fs.mkdirSync(path.dirname(file), { recursive: true });
let measurement = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, 'utf8')) : null;
const plan = path.join(root, 'imp-plan.html');
const html = fs.readFileSync(plan, 'utf8');
const pattern = /(<script type="application\/json" id="agent-ledger">\s*)([\s\S]*?)(\s*<\/script>)/;
const ledger = JSON.parse(html.match(pattern)[2]);
const item = ledger.items[taskId];
if (action === 'start') {
  const window = { turnId: turn.payload.turn_id, startedAt: now, baseline: { at: latest.timestamp, usage: latest.payload.info.total_token_usage } };
  if (!measurement) measurement = { schemaVersion: 1, taskId, threadId: rows.find(row => row.type === 'session_meta').payload.session_id, model: turn.payload.model, reasoningEffort: turn.payload.effort, ...window, calls: [] };
  if (measurement.windows) measurement.windows.push(window);
  if (measurement.usageEndedAt) { measurement.windows = [{ startedAt: measurement.startedAt, baseline: measurement.baseline, usageEndedAt: measurement.usageEndedAt }, window]; delete measurement.usageEndedAt; }
  measurement.status = 'active';
  item.state = 'active';
  item.periods.push({ from: now, to: null });
}
if (action === 'wait' || action === 'done') {
  assert.ok(measurement);
  if (measurement.status === 'active') {
    const window = measurement.windows?.at(-1) ?? measurement;
    window.usageEndedAt = latest.timestamp;
  }
  measurement.status = action === 'done' ? 'done' : 'waiting';
  item.state = action === 'done' ? 'done' : 'verification';
  if (action === 'done') measurement.completedAt = now;
  const period = item.periods.at(-1);
  if (period && period.to === null) period.to = now;
}
assert.ok(measurement);
measurement.updatedAt = now;
item.log.push({ at: now, note, ...(evidence ? { evidence } : {}) });
ledger.asOf = now;
fs.writeFileSync(file, JSON.stringify(measurement, null, 2) + '\n');
fs.writeFileSync(plan, html.replace(pattern, (_, before, old, after) => before + JSON.stringify(ledger, null, 2) + after));
const result = recordUsage(sessionPath, taskId);
console.log(taskId + ' ' + measurement.status + ', ' + result.calls.length + ' usage records.');
