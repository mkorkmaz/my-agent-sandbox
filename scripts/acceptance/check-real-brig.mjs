import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { runtimeConfigurationFile } from './acceptance-runtime.mjs';

const workspace = fileURLToPath(new URL('../../', import.meta.url));
const brigPath = process.argv[3] ?? path.join(workspace, '.build/brig-runtime/bin/brig');
const runtime = process.argv[4] ?? path.join(workspace, '.build/brig-runtime/bin/hull');
const evidence = path.join(workspace, 'artifacts/bd-15/real-brig.json');
const root = process.argv[2] ?? fs.mkdtempSync('/private/tmp/brig-bd15-');
assert.ok(root.startsWith('/private/tmp/brig-bd15-') && fs.realpathSync(root) === root && fs.statSync(root).uid === process.getuid());
fs.chmodSync(root, 0o700);
const label = 'bd15-' + path.basename(root).split('-').at(-1).toLowerCase();
const reference = 'codex@' + label;
const project = path.join(root, 'sample-project');
const second = path.join(root, 'second project');
const home = path.join(root, 'home');
for (const directory of [project, second, home, 'state', 'config', 'policies', 'tmp'].map(value => path.isAbsolute(value) ? value : path.join(root, value))) fs.mkdirSync(directory, { recursive: true });
fs.writeFileSync(path.join(project, 'README.md'), 'Temporary Tugboat acceptance project.\n');
fs.writeFileSync(path.join(second, 'sentinel.txt'), 'Preserve the second project.\n');
const environment = {
  HOME: home, USER: os.userInfo().username, LOGNAME: os.userInfo().username,
  PATH: [path.dirname(brigPath), path.dirname(runtime), '/usr/bin', '/bin', '/usr/sbin', '/sbin'].join(':'), LANG: 'en_US.UTF-8',
  TMPDIR: root + '/tmp/', XDG_CONFIG_HOME: root + '/config',
  BRIG_STATE_DIR: root + '/state', BRIG_POLICY_DIR: root + '/policies',
  BRIG_HYPERVISOR: 'hvi', BRIG_RUNTIME: 'hull', BRIG_RUNTIME_BIN: runtime, BRIG_VERIFY: 'require',
  BRIG_NETWORK: 'isolated', DO_NOT_TRACK: '1', TERM: 'xterm-256color',
  MKE2FS_CONFIG: runtimeConfigurationFile(runtime),
};
const report = { schemaVersion: 1, startedAt: new Date().toISOString(), root, reference, isolatedHome: true, verification: 'require', userInterfaceTest: false, accountTest: false, commands: [], checks: [], outcome: 'running' };
function save() { fs.writeFileSync(evidence, JSON.stringify(report, null, 2) + '\n'); }
function command(binary, args, timeout = 30_000) {
  const started = Date.now();
  const result = spawnSync(binary, args, { env: environment, cwd: project, encoding: 'utf8', timeout, maxBuffer: 1_048_576, input: '' });
  const record = { binary, args, exitCode: result.status, signal: result.signal, error: result.error?.code ?? null, durationMs: Date.now() - started, stdout: result.stdout ?? '', stderr: result.stderr ?? '' };
  report.commands.push(record); save();
  console.log(path.basename(binary), args.slice(0, 3).join(' '), 'exit', result.status, record.error ?? '');
  return record;
}
function brig(args, timeout) { return command(brigPath, args, timeout); }
function checked(args, timeout) {
  const result = brig(args, timeout); assert.equal(result.exitCode, 0, result.stderr || result.stdout || result.error || result.signal); return result;
}
function json(args) { return JSON.parse(checked(args).stdout); }
function check(name, passed, observed) {
  report.checks.push({ name, passed, observed }); save(); assert.ok(passed, name + ': ' + observed);
}
const policy = label + '-network';
const secondPolicy = label + '-other';
const editor = path.join(root, 'editor');
const source = path.join(root, 'policy.yaml');
fs.writeFileSync(editor, '#!/bin/sh\nexec /bin/cp "$BRIG_DESKTOP_POLICY_SOURCE" "$1"\n', { mode: 0o700 });
environment.EDITOR = editor; environment.VISUAL = editor; environment.BRIG_DESKTOP_POLICY_SOURCE = source;
function createPolicy(name, session) {
  fs.writeFileSync(source, `apiVersion: brig.sh/v1alpha1\nkind: Policy\nmetadata:\n  name: ${name}\nspec:\n  egress:\n    default: deny\n    allow:\n      - host: auth.openai.com\n      - host: chatgpt.com\n      - host: api.openai.com\n`);
  if (!fs.existsSync(root + '/policies/' + name + '.yaml')) checked(['policy', 'create', name]);
  checked(['policy', 'attach', name, 'codex', '-n', session]);
}
const passed = 'passed-with-native-and-account-acceptance-pending';
let guestNeedsCleanup = false;
fs.mkdirSync(path.dirname(evidence), { recursive: true });
save();
try {
  json(['--json', 'version']);
  const initial = json(['--json', 'ls']);
  check('isolated-empty-inventory', initial.data.sandboxes.length === 0, initial.data.sandboxes);
  createPolicy(policy, label); createPolicy(secondPolicy, label + '-other');
  const secondHash = createHash('sha256').update(fs.readFileSync(root + '/policies/' + secondPolicy + '.yaml')).digest('hex');
  const planArgs = ['--network', 'isolated', '--cpus', '2', '--mem', '2048', reference, project];
  const plan = json(['--json', 'plan', ...planArgs]);
  check('exact-project-plan', plan.data.project.host === project && plan.data.runtime.backend === 'hvi' && plan.data.runtime.available && plan.data.refused == null, plan.data);
  check('no-host-credentials', plan.data.credentials.every(value => ['unresolved', 'withheld'].includes(value.state)), plan.data.credentials);
  check('required-image-verification', plan.data.verify.mode === 'require', plan.data.verify);
  guestNeedsCleanup = true;
  const boot = brig(['run', ...planArgs, '--', '--version'], 240_000);
  check('real-hvi-codex-version', boot.exitCode === 0 && /codex/i.test(boot.stdout), boot.stderr || boot.stdout);
  const info = json(['--json', 'info', reference]);
  check('info-project-home', info.data.project.host === project && info.data.workspace.startsWith(root + '/state/'), info.data);
  const kernel = checked(['sh', reference, 'uname', '-s']);
  check('linux-guest', kernel.stdout.trim() === 'Linux', kernel.stdout);
  checked(['sh', reference, 'sh', '-c', 'printf bd15-guest-write > "$BRIG_PROJECT/from-guest.txt"']);
  check('guest-project-write', fs.readFileSync(project + '/from-guest.txt', 'utf8') === 'bd15-guest-write', 'Host readback');
  const outside = path.join(root, 'outside-canary.txt'); fs.writeFileSync(outside, 'Host-only canary.');
  check('outside-host-file-denied', brig(['sh', reference, 'cat', outside]).exitCode !== 0, outside);
  const allowed = brig(['sh', reference, 'curl', '-q', '--noproxy', '*', '-I', '--connect-timeout', '5', '--max-time', '10', '-sS', 'https://auth.openai.com']);
  const hostControl = command('/usr/bin/curl', ['-q', '--noproxy', '*', '-I', '--connect-timeout', '5', '--max-time', '10', '-sS', 'https://example.com']);
  const denied = brig(['sh', reference, 'curl', '-q', '--noproxy', '*', '-I', '--connect-timeout', '5', '--max-time', '10', '-sS', 'https://example.com']);
  check('allowed-and-denied-network', allowed.exitCode === 0 && hostControl.exitCode === 0 && [6, 7, 28].includes(denied.exitCode), { allowed: allowed.exitCode, host: hostControl.exitCode, denied: denied.exitCode });
  checked(['sh', reference, 'codex', 'login', '--help']);
  checked(['sh', reference, 'codex', 'resume', '--help']);
  checked(['stop', reference]);
  checked(['run', ...planArgs, '--', '--version'], 60_000);
  check('stop-restart-preserves-home', json(['--json', 'info', reference]).data.workspace === info.data.workspace, info.data.workspace);
  checked(['stop', reference]);
  const dryRun = checked(['rm', reference, '--dry-run']);
  check('exact-removal-preview', dryRun.stdout.includes(reference), dryRun.stdout);
  checked(['rm', reference]); guestNeedsCleanup = false;
  check('second-project-preserved', fs.readFileSync(second + '/sentinel.txt', 'utf8') === 'Preserve the second project.\n' && secondHash === createHash('sha256').update(fs.readFileSync(root + '/policies/' + secondPolicy + '.yaml')).digest('hex'), 'Second folder and policy hashes match');
  report.outcome = passed;
} catch (error) {
  report.outcome = 'blocked'; report.blocker = String(error.message);
} finally {
  const errors = [];
  if (guestNeedsCleanup) {
    try {
      if (json(['--json', 'ls']).data.sandboxes.some(value => value.ref === reference)) {
        const stop = brig(['stop', reference]);
        if (stop.exitCode !== 0) errors.push('Could not stop ' + reference);
        const preview = checked(['rm', reference, '--dry-run']);
        assert.ok(preview.stdout.includes(reference), 'Cleanup preview did not name ' + reference);
        checked(['rm', reference]);
      }
    } catch (error) { errors.push(error.message); }
  }
  try {
    report.remaining = json(['--json', 'ls']).data.sandboxes;
    if (report.remaining.some(value => value.ref === reference)) errors.push('Guest remains: ' + reference);
  } catch (error) { errors.push('Could not verify final inventory: ' + error.message); }
  report.storeDetach = command(runtime, ['store', 'detach']);
  if (report.storeDetach.exitCode !== 0) errors.push('Could not detach the isolated hull store');
  if (errors.length) { report.outcome = 'blocked'; report.cleanupError = errors.join('; '); }
  process.exitCode = report.outcome === passed ? 0 : 1;
  report.completedAt = new Date().toISOString(); save();
  console.log('Outcome:', report.outcome, 'Evidence:', evidence);
}
