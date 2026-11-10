import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export function normalizeUsage(raw) {
  const input = raw.input_tokens;
  const reasoning = raw.reasoning_output_tokens ?? 0;
  let output = raw.output_tokens;
  assert.ok(Number.isSafeInteger(input) && input >= 0);
  assert.ok(Number.isSafeInteger(output) && output >= 0);
  if (raw.total_tokens === input + output + reasoning) output += reasoning;
  assert.equal(raw.total_tokens, input + output, 'Token total does not reconcile');
  const write = raw.cache_write_input_tokens ?? null;
  return { input, read: raw.cached_input_tokens ?? null, write, output };
}

export function collectUsage(rows, measurement) {
  let previous = normalizeUsage(measurement.baseline.usage);
  let insideTurn = false;
  let complete = false;
  let modelVerified = false;
  const calls = [];
  const anomalies = [];
  let through = measurement.startedAt;
  for (const row of rows) {
    const payload = row.payload;
    if (row.type === 'event_msg' && payload.type === 'task_started') {
      insideTurn = payload.turn_id === measurement.turnId;
    }
    if (!insideTurn) continue;
    if (row.type === 'turn_context') {
      assert.equal(payload.model, measurement.model, 'Model changed within the measured turn');
      modelVerified = true;
    }
    if (row.type === 'event_msg' && payload.type === 'task_complete' && payload.turn_id === measurement.turnId) {
      complete = true;
      through = row.timestamp;
      insideTurn = false;
    }
    if (row.type !== 'event_msg' || payload.type !== 'token_count' || !payload.info) continue;
    // A continuous implementation turn can contain several task windows.
    if (measurement.baseline.at && row.timestamp <= measurement.baseline.at) continue;
    if (measurement.usageEndedAt && row.timestamp > measurement.usageEndedAt) break;
    const current = normalizeUsage(payload.info.total_token_usage);
    const delta = Object.fromEntries(Object.entries(current).map(([key, value]) => [
      key, value === null || previous[key] === null ? null : value - previous[key],
    ]));
    previous = current;
    if (delta.input === 0 && delta.output === 0) continue;
    const last = normalizeUsage(payload.info.last_token_usage);
    const reset = Object.values(delta).some(value => value !== null && value < 0);
    const usage = reset ? last : delta;
    if (reset) anomalies.push('Counter reset at ' + row.timestamp + '; last request retained, coverage partial.');
    assert.ok(usage.input >= (usage.read ?? 0) + (usage.write ?? 0), 'Input classes overlap');
    const matchesRequest = Object.keys(last).every(key => last[key] === usage[key]);
    calls.push({
      key: measurement.turnId + '/' + row.timestamp,
      kind: matchesRequest ? 'request' : 'window',
      ...usage,
      write12: usage.write === 0 ? 0 : null,
    });
    through = row.timestamp;
    if (measurement.usageEndedAt === row.timestamp) complete = true;
  }
  assert.ok(modelVerified, 'Measured turn context is unavailable');
  return { calls, through, complete, anomalies };
}

function atomicJson(file, value) {
  const temporary = file + '.tmp';
  fs.writeFileSync(temporary, JSON.stringify(value, null, 2) + '\n');
  fs.renameSync(temporary, file);
}

export function recordUsage(sessionPath, taskId) {
  assert.match(taskId, /^BD-\d{2}$/);
  const project = fileURLToPath(new URL('../', import.meta.url));
  const measurementPath = path.join(project, 'artifacts', taskId.toLowerCase(), 'measurement.json');
  const measurement = JSON.parse(fs.readFileSync(measurementPath, 'utf8'));
  assert.equal(measurement.taskId, taskId);
  const result = readUsage(sessionPath, measurement);
  const contributors = [];
  for (const delegation of measurement.delegations ?? []) {
    assert.equal(delegation.model, measurement.model, 'Delegated model requires a separate price schedule');
    const contribution = readUsage(delegation.sessionFile, delegation);
    result.calls.push(...contribution.calls);
    result.complete = result.complete && contribution.complete;
    result.anomalies.push(...contribution.anomalies);
    if (contribution.through > result.through) result.through = contribution.through;
    contributors.push({ role: delegation.role, turnId: delegation.turnId, complete: contribution.complete });
  }
  for (const contributor of measurement.unmeasuredContributors ?? []) {
    result.anomalies.push('Usage unavailable for ' + contributor.role + ': ' + contributor.reason);
    contributors.push({ role: contributor.role, complete: false, reason: contributor.reason });
  }
  measurement.calls = result.calls;
  measurement.accounting = {
    through: result.through,
    turnComplete: result.complete,
    anomalies: result.anomalies,
    contributors,
  };
  measurement.updatedAt = new Date().toISOString();

  const planPath = path.join(project, 'imp-plan.html');
  const html = fs.readFileSync(planPath, 'utf8');
  const pattern = /(<script type="application\/json" id="agent-ledger">\s*)([\s\S]*?)(\s*<\/script>)/;
  const match = html.match(pattern);
  assert.ok(match, 'Plan ledger is unavailable');
  const ledger = JSON.parse(match[2]);
  const item = ledger.items[taskId];
  assert.ok(item, 'Unknown task');
  item.usage = result.calls;
  item.coverage = result.calls.length ? 'partial' : 'none';
  if (result.complete && result.anomalies.length === 0) item.coverage = 'full';
  ledger.asOf = measurement.updatedAt;
  const changed = html.replace(pattern, (_, before, content, after) => before + JSON.stringify(ledger, null, 2) + after);
  atomicJson(measurementPath, measurement);
  const temporaryPlan = planPath + '.tmp';
  fs.writeFileSync(temporaryPlan, changed);
  fs.renameSync(temporaryPlan, planPath);
  return result;
}

function readUsage(sessionPath, measurement) {
  const raw = fs.readFileSync(sessionPath, 'utf8');
  // A live writer may not have finished its final JSONL line yet.
  const lines = raw.split('\n');
  lines.pop();
  const rows = lines.filter(Boolean).map(line => JSON.parse(line));
  if (!measurement.windows) return collectUsage(rows, measurement);
  const results = measurement.windows.map(window => collectUsage(rows, { ...measurement, ...window, windows: undefined }));
  return {
    calls: results.flatMap(result => result.calls),
    through: results.map(result => result.through).sort().at(-1) ?? measurement.startedAt,
    complete: results.every(result => result.complete),
    anomalies: results.flatMap(result => result.anomalies),
  };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [sessionPath, taskId, mode] = process.argv.slice(2);
  assert.ok(sessionPath && taskId, 'Usage: node scripts/record-task-usage.mjs SESSION_JSONL BD-NN [--follow]');
  assert.ok(mode === undefined || mode === '--follow');
  const first = recordUsage(sessionPath, taskId);
  console.log(taskId + ': ' + first.calls.length + ' usage records, turn complete ' + first.complete);
  if (mode === '--follow' && !first.complete) {
    const started = Date.now();
    const timer = setInterval(() => {
      try {
        const latest = recordUsage(sessionPath, taskId);
        if (latest.complete) {
          clearInterval(timer);
          console.log(taskId + ': accounting finalized after the last response.');
        }
        if (Date.now() - started > 10 * 60 * 1000) {
          clearInterval(timer);
          process.exitCode = 1;
          console.error('Accounting follow timed out. Run the same command again to reconcile.');
        }
      } catch (error) {
        clearInterval(timer);
        process.exitCode = 1;
        console.error(error.message);
      }
    }, 1000);
  }
}
