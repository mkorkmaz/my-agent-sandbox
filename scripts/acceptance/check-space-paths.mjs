import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { runtimeConfigurationFile } from './acceptance-runtime.mjs';

const workspace = fileURLToPath(new URL('../../', import.meta.url));
const brigPath = process.argv[4] ?? path.join(workspace, '.build/brig-runtime/bin/brig');
const cacheHome = process.argv[2];
assert.ok(cacheHome && fs.realpathSync(cacheHome).startsWith('/private/tmp/brig-'), 'Pass an isolated Brig cache HOME.');
const root = process.argv[3] ?? fs.mkdtempSync('/private/tmp/brig-space-');
assert.ok(root.startsWith('/private/tmp/brig-space-') && fs.realpathSync(root) === root && fs.statSync(root).uid === process.getuid());
fs.chmodSync(root, 0o700);
const project = path.join(root, 'parent folder/sample project');
const second = path.join(root, 'other project');
for (const directory of [project, second, ...['state', 'config', 'policies', 'tmp'].map(value => path.join(root, value))]) fs.mkdirSync(directory, { recursive: true });
fs.writeFileSync(project + '/from host.txt', 'host-to-guest with spaces\n');
fs.writeFileSync(second + '/preserve.txt', 'unshared project\n');
const sentinel = createHash('sha256').update(fs.readFileSync(second + '/preserve.txt')).digest('hex');
const label = 'bd-sp-' + path.basename(root).split('-').at(-1).toLowerCase();
const reference = 'codex@' + label;
const runtime = process.argv[5] ?? path.join(workspace, '.build/brig-runtime/bin/hull');
assert.ok(fs.existsSync(runtime), 'Prepare the patched hull first.');
const env = {
  HOME: cacheHome, USER: process.env.USER, LOGNAME: process.env.USER,
  PATH: [path.dirname(brigPath), path.dirname(runtime), '/usr/bin', '/bin', '/usr/sbin', '/sbin'].join(':'), LANG: 'en_US.UTF-8', TERM: 'xterm-256color',
  TMPDIR: root + '/tmp/', XDG_CONFIG_HOME: root + '/config', BRIG_STATE_DIR: root + '/state', BRIG_POLICY_DIR: root + '/policies',
  BRIG_HYPERVISOR: 'hvi', BRIG_RUNTIME: 'hull', BRIG_RUNTIME_BIN: runtime, BRIG_VERIFY: 'require', BRIG_NETWORK: 'isolated', DO_NOT_TRACK: '1',
  MKE2FS_CONFIG: runtimeConfigurationFile(runtime),
};
const report = { startedAt: new Date().toISOString(), root, project, reference, cacheHome, stockBrig: brigPath, runtime, commands: [], checks: [], outcome: 'running' };
const evidence = path.join(workspace, 'artifacts/space-paths/real-brig.json');
const save = () => fs.writeFileSync(evidence, JSON.stringify(report, null, 2) + '\n');
function run(binary, args, timeout = 30_000) {
  const started = Date.now();
  const result = spawnSync(binary, args, { cwd: project, env, input: '', encoding: 'utf8', timeout, maxBuffer: 1_048_576 });
  const record = { binary, args, status: result.status, signal: result.signal, error: result.error?.code, stdout: result.stdout ?? '', stderr: result.stderr ?? '', milliseconds: Date.now() - started };
  report.commands.push(record); save();
  assert.equal(record.status, 0, record.stderr || record.stdout || record.error);
  return record.stdout;
}
const brig = (args, timeout) => run(brigPath, args, timeout);
const json = args => JSON.parse(brig(['--json', ...args]));
function check(name, passed, observed) { report.checks.push({ name, passed, observed }); save(); assert.ok(passed, name); }
const options = ['--network', 'isolated', '--cpus', '2', '--mem', '2048', reference, project];
const editor = path.join(root, 'editor');
const policy = label + '-network';
fs.writeFileSync(root + '/policy.yaml', 'apiVersion: brig.sh/v1alpha1\nkind: Policy\nmetadata:\n  name: ' + policy + '\nspec:\n  egress:\n    default: deny\n    allow:\n      - host: api.openai.com\n      - host: auth.openai.com\n      - host: chatgpt.com\n');
fs.writeFileSync(editor, '#!/bin/sh\nexec /bin/cp "$BRIG_DESKTOP_POLICY_SOURCE" "$1"\n', { mode: 0o700 });
env.EDITOR = editor; env.VISUAL = editor; env.BRIG_DESKTOP_POLICY_SOURCE = root + '/policy.yaml';
let owned = false;
fs.mkdirSync(path.dirname(evidence), { recursive: true });
save();
try {
  check('stock-brig-version', json(['version']).data.version === 'v0.5.2', 'Installed Brig v0.5.2; no modified Brig binary.');
  report.initialReferences = json(['ls']).data.sandboxes.map(value => value.ref);
  assert.ok(!report.initialReferences.includes(reference), 'The test session must be new.');
  brig(['policy', 'create', policy]);
  brig(['policy', 'attach', policy, 'codex', '-n', label]);
  const plan = json(['plan', ...options]);
  check('original-spaced-host-and-guest-path', plan.data.project.host === project && plan.data.project.guest === '/work/sample project', plan.data.project);
  check('patched-runtime-and-required-verification', plan.data.runtime.bin === runtime && plan.data.runtime.backend === 'hvi' && plan.data.verify.mode === 'require' && plan.data.egress.default === 'deny' && plan.data.refused == null, { runtime: plan.data.runtime, verification: plan.data.verify });
  owned = true;
  const version = brig(['run', ...options, '--', '--version'], 240_000);
  check('real-codex-boot', /codex/i.test(version), version.trim());
  const info = json(['info', reference]);
  check('info-preserves-original-path', info.data.project.host === project && info.data.project.guest === '/work/sample project' && info.data.workspace.startsWith(root + '/state/'), info.data.project);
  check('linux-guest', brig(['sh', reference, 'uname', '-s']).trim() === 'Linux', 'Linux');
  const readback = brig(['sh', reference, 'sh', '-c', 'printf "%s\\n" "$BRIG_PROJECT"; pwd; cat "$BRIG_PROJECT/from host.txt"; printf "guest-to-host with spaces\\n" > "$BRIG_PROJECT/from guest.txt"; printf retained > /root/space-path-retained.txt']);
  check('guest-cwd-and-host-file', readback.replaceAll('\r\n', '\n').includes('/work/sample project\n/work/sample project\nhost-to-guest with spaces'), readback);
  check('guest-file-visible-in-original-folder', fs.readFileSync(project + '/from guest.txt', 'utf8') === 'guest-to-host with spaces\n', 'Host readback in the unchanged spaced folder.');
  run('/usr/bin/git', ['init', '--quiet', project]);
  const status = brig(['sh', reference, 'git', '--no-pager', '-C', '/work/sample project', 'status', '--short']);
  check('git-with-spaced-worktree', status.includes('from guest.txt') && status.includes('from host.txt'), status);
  brig(['stop', reference]);
  brig(['run', ...options, '--', '--version'], 90_000);
  const reopened = brig(['sh', reference, 'sh', '-c', 'printf "%s\\n" "$BRIG_PROJECT"; pwd; cat "$BRIG_PROJECT/from guest.txt"; cat /root/space-path-retained.txt']);
  check('stop-and-reopen-preserves-home-and-share', reopened.replaceAll('\r\n', '\n').includes('/work/sample project\n/work/sample project\nguest-to-host with spaces\nretained') && json(['info', reference]).data.workspace === info.data.workspace, reopened);
  check('other-project-unchanged', sentinel === createHash('sha256').update(fs.readFileSync(second + '/preserve.txt')).digest('hex'), sentinel);
  report.outcome = 'passed';
} catch (error) {
  report.outcome = 'failed'; report.failure = error.message; process.exitCode = 1;
} finally {
  if (owned) {
    try {
      const info = json(['info', reference]);
      assert.ok(info.data.workspace.startsWith(root + '/state/'), 'Cleanup must stay in the test guest home.');
      brig(['stop', reference]);
      const preview = brig(['rm', reference, '--dry-run']); assert.ok(preview.includes(reference));
      brig(['rm', reference]);
    } catch (error) { report.outcome = 'failed'; report.cleanupError = error.message; process.exitCode = 1; }
  }
  try {
    report.finalReferences = json(['ls']).data.sandboxes.map(value => value.ref);
    check('preexisting-environments-preserved', report.initialReferences.every(value => report.finalReferences.includes(value)), report.finalReferences);
  } catch (error) { report.outcome = 'failed'; report.inventoryError = error.message; process.exitCode = 1; }
  report.completedAt = new Date().toISOString(); save();
  console.log(JSON.stringify({ outcome: report.outcome, checks: report.checks.length, evidence, cleanupError: report.cleanupError, failure: report.failure }));
}
