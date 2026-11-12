import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const workspace = fileURLToPath(new URL('../', import.meta.url));
const root = process.argv[2] ?? fs.mkdtempSync('/private/tmp/tba-');
assert.ok(root.startsWith('/private/tmp/tba-') && fs.realpathSync(root) === root && fs.statSync(root).uid === process.getuid());
const reference = 'codex@a01';
const runtime = path.join(workspace, '.build/hull-compat/bin/hull');
const project = path.join(root, 'project');
const evidence = path.join(workspace, 'artifacts/shared-chatgpt/compatibility.json');
const report = { schemaVersion: 1, startedAt: new Date().toISOString(), root, reference,
  syntheticCredentials: true, actualAccountTest: false, outcome: 'running', commands: [] };
fs.chmodSync(root, 0o700);
for (const name of ['project', 'home', 'state', 'config', 'policies', 'tmp', 'guest']) fs.mkdirSync(path.join(root, name), { recursive: true });
fs.mkdirSync(path.dirname(evidence), { recursive: true });
fs.copyFileSync(path.join(workspace, 'tests/fixtures/shared-chatgpt/guest-compat.py'), path.join(project, 'guest-compat.py'));
const environment = {
  HOME: path.join(root, 'home'), USER: os.userInfo().username, LOGNAME: os.userInfo().username,
  PATH: path.dirname(runtime) + ':/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin',
  LANG: 'en_US.UTF-8', TERM: 'dumb', TMPDIR: root + '/tmp/', XDG_CONFIG_HOME: root + '/config',
  BRIG_STATE_DIR: root + '/state', BRIG_POLICY_DIR: root + '/policies', BRIG_HYPERVISOR: 'hvi',
  BRIG_RUNTIME: 'hull', BRIG_RUNTIME_BIN: runtime, BRIG_VERIFY: 'require', BRIG_NETWORK: 'isolated', DO_NOT_TRACK: '1',
};
function save() { fs.writeFileSync(evidence, JSON.stringify(report, null, 2) + '\n'); }
async function command(binary, args, timeout = 30_000) {
  const started = Date.now();
  const child = spawn(binary, args, { env: environment, cwd: project, stdio: ['ignore', 'pipe', 'pipe'] });
  let stdout = '', stderr = '';
  child.stdout.on('data', bytes => { stdout += bytes; });
  child.stderr.on('data', bytes => { stderr += bytes; });
  const timer = setTimeout(() => child.kill('SIGTERM'), timeout);
  const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', resolve); });
  clearTimeout(timer);
  report.commands.push({ binary, args, exitCode: code, durationMs: Date.now() - started, stdout, stderr });
  save(); console.log(path.basename(binary), args.slice(0, 3).join(' '), 'exit', code);
  return { code, stdout, stderr };
}
const brig = (args, timeout) => command('/opt/homebrew/bin/brig', args, timeout);
async function checked(args, timeout) { const result = await brig(args, timeout); assert.equal(result.code, 0, result.stderr); return result; }
const runArguments = ['--network', 'isolated', '--cpus', '2', '--mem', '2048', '--home', root + '/guest', reference, project];
let created = false;
save();
try {
  await checked(['version']);
  const list = JSON.parse((await checked(['--json', 'ls'])).stdout);
  assert.equal(list.data.sandboxes.length, 0);
  const editor = path.join(root, 'edit-policy');
  fs.writeFileSync(editor, '#!/bin/sh\nexec /bin/cp "$TUGBOAT_TEST_POLICY" "$1"\n', { mode: 0o700 });
  const policy = path.join(root, 'policy.yaml');
  fs.writeFileSync(policy, 'apiVersion: brig.sh/v1alpha1\nkind: Policy\nmetadata:\n  name: a01-network\nspec:\n  egress:\n    default: deny\n    allow:\n      - host: auth.openai.com\n      - host: chatgpt.com\n      - host: api.openai.com\n');
  Object.assign(environment, { EDITOR: editor, VISUAL: editor, TUGBOAT_TEST_POLICY: policy });
  if (!fs.existsSync(root + '/policies/a01-network.yaml')) await checked(['policy', 'create', 'a01-network']);
  await checked(['policy', 'attach', 'a01-network', 'codex', '-n', 'a01']);
  const plan = JSON.parse((await checked(['--json', 'plan', ...runArguments])).stdout);
  assert.equal(plan.data.runtime.backend, 'hvi');
  assert.equal(plan.data.project.host, project);
  assert.equal(plan.data.verify.mode, 'require');
  assert.equal(plan.data.egress.default, 'deny');
  assert.ok(plan.data.credentials.every(value => ['unresolved', 'withheld'].includes(value.state)));
  created = true;
  await checked(['run', ...runArguments, '--', '--version'], 240_000);
  await checked(['sh', reference, 'sh', '-c', 'command -v python3; command -v codex; printf "HOME=%s\\nCODEX_HOME=%s\\n" "$HOME" "${CODEX_HOME:-default}"']);
  const probe = await checked(['sh', reference, 'python3', '/work/project/guest-compat.py'], 210_000);
  report.probe = JSON.parse(probe.stdout);
  assert.ok(report.probe.cases.every(value => value.passed));
  report.outcome = 'passed-synthetic-guest';
} catch (error) { report.outcome = 'blocked'; report.error = error.message; }
finally {
  if (created) {
    await brig(['stop', reference]);
    const dry = await brig(['rm', reference, '--dry-run']);
    if (dry.code === 0 && dry.stdout.includes(reference)) await brig(['rm', reference]);
  }
  const inventory = await brig(['--json', 'ls']);
  if (inventory.code === 0) report.remaining = JSON.parse(inventory.stdout).data.sandboxes;
  if (report.remaining?.length) { report.outcome = 'blocked'; report.cleanupError = 'Temporary environments remain.'; }
  await command(runtime, ['store', 'detach']);
  report.completedAt = new Date().toISOString(); save();
  console.log('Outcome:', report.outcome, 'Evidence:', evidence);
  process.exitCode = report.outcome === 'passed-synthetic-guest' ? 0 : 1;
}
