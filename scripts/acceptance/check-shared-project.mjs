import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runAcceptanceProcess } from './acceptance-process.mjs';
import { runtimeConfigurationFile } from './acceptance-runtime.mjs';
import { cleanSharedProject } from './shared-project-cleanup.mjs';

const workspace = fileURLToPath(new URL('../../', import.meta.url));
const runtime = path.join(workspace, '.build/brig-runtime/bin');
const archive = 'codex-package-aarch64-unknown-linux-musl.tar.gz';
const release = 'rust-v0.171.0';
const releaseUrl = `https://github.com/openai/codex/releases/download/${release}`;
const archiveDigest = 'd8c43bee6cf11b01eb18b95f4d7961092bc40228180808859e70dabb7784c1c9';
const bundleDigest = 'ad7a3d4d1703a6c664c222d77c462a3932072bba5dfced64677d148cbbe640ff';
const profile = 'tugboat-project-proof';
const reference = `${profile}@proof`;
const sandbox = `brig-${profile}-proof`;
const keep = process.argv.slice(2).join(' ') === '--keep';
assert.ok(process.argv.length === 2 || keep, 'Usage: node check-shared-project.mjs [--keep]');
assert.equal(`${process.platform}/${process.arch}`, 'darwin/arm64');
for (const tool of ['brig', 'hull', 'cosign']) fs.accessSync(path.join(runtime, tool), fs.constants.X_OK);

const root = fs.mkdtempSync('/private/tmp/tgp1.');
const project = path.join(root, 'sample project');
const runner = path.join(root, 'run');
const evidence = path.join(root, 'evidence.json');
const report = { reference, root, checks: [], commands: [] };
const quote = value => `'${value.replaceAll("'", "'\"'\"'")}'`;
function save() { fs.writeFileSync(evidence, JSON.stringify(report, null, 2) + '\n', { mode: 0o600 }); }
function check(name, detail) { report.checks.push({ name, detail }); save(); console.log(name); }

for (const directory of ['home', 'registry', 'state', 'config', 'profiles', 'policies', 'tmp', 'downloads', 'sample project']) {
  fs.mkdirSync(path.join(root, directory), { mode: 0o700 });
}
const username = os.userInfo().username;
const environment = {
  HOME: `${root}/home`, USER: username, LOGNAME: username,
  PATH: `${runtime}:/usr/bin:/bin:/usr/sbin:/sbin`, LANG: 'en_US.UTF-8', TERM: 'xterm-256color',
  TMPDIR: `${root}/tmp/`, XDG_CONFIG_HOME: `${root}/config`, DOCKER_CONFIG: `${root}/registry`,
  BRIG_STATE_DIR: `${root}/state`, BRIG_PROFILE_DIR: `${root}/profiles`, BRIG_POLICY_DIR: `${root}/policies`,
  BRIG_RUNTIME: 'hull', BRIG_RUNTIME_BIN: `${runtime}/hull`, BRIG_VERIFY: 'require', DO_NOT_TRACK: '1',
  MKE2FS_CONFIG: runtimeConfigurationFile(`${runtime}/hull`),
};
const bindings = Object.entries(environment).map(([name, value]) => `${name}=${quote(value)}`).join(' ');
fs.writeFileSync(runner, `#!/bin/sh
set -eu
tool="$1"
shift
case "$tool" in brig|hull|cosign) ;; *) exit 2 ;; esac
cd ${quote(project)}
exec /usr/bin/env -i ${bindings} ${quote(runtime)}/"$tool" "$@"
`, { mode: 0o700 });
fs.writeFileSync(path.join(root, 'registry/config.json'), '{}\n');
fs.writeFileSync(path.join(project, 'README.md'), 'Disposable shared Tugboat project.\n');
fs.copyFileSync(new URL('./shared-project-profile.yaml', import.meta.url), `${root}/profiles/${profile}.yaml`);
fs.copyFileSync(new URL('./shared-project-policy.yaml', import.meta.url), `${root}/policies/${profile}.yaml`);
console.log(`Evidence file ${evidence}`);
save();

async function run(binary, args, timeout = 30_000) {
  const result = await runAcceptanceProcess(binary, args, { env: environment, cwd: project, timeout });
  report.commands.push({ binary, args, ...result });
  save();
  return result;
}
async function checked(binary, args, timeout) {
  const result = await run(binary, args, timeout);
  assert.equal(result.status, 0, `${binary} ${args[0]} failed. See ${evidence}.`);
  return result;
}
const brig = (args, timeout) => checked(runner, ['brig', ...args], timeout);
async function json(args, kind) {
  const envelope = JSON.parse((await brig(['--json', ...args])).stdout);
  assert.equal(envelope.apiVersion, 'brig.sh/v1alpha1');
  assert.equal(envelope.kind, kind);
  return envelope.data;
}
const guest = (...args) => brig(['exec', reference, '--', ...args]);
const runArguments = ['--network', 'isolated', reference, project];
const codex = '/root/.local/share/tugboat-phase1/codex/bin/codex';
async function boot() {
  const output = await brig(['--verbose', 'run', '-d', ...runArguments], 720_000);
  assert.match(output.stderr, /image and boot assets verified/);
  const inventory = await json(['ls'], 'SandboxList');
  assert.equal(inventory.sandboxes.length, 1);
  assert.equal(inventory.sandboxes[0].ref, reference);
  assert.equal(inventory.sandboxes[0].state, 'running');
  return (await guest('cat', '/proc/sys/kernel/random/boot_id')).stdout.trim();
}

try {
  const version = await json(['version'], 'Version');
  assert.equal(version.version, 'v0.5.2');
  assert.deepEqual((await json(['ls'], 'SandboxList')).sandboxes, []);
  await brig(['policy', 'attach', profile, profile, '-n', 'proof']);
  const plan = await json(['plan', ...runArguments], 'Plan');
  assert.equal(plan.refused, undefined);
  assert.equal(plan.runtime.backend, 'hvi');
  assert.deepEqual(plan.verify, { mode: 'require', policy: 'brig' });
  assert.deepEqual(plan.credentials, []);
  assert.deepEqual(plan.ports, []);
  assert.deepEqual(plan.policies, [profile]);
  assert.equal(plan.egress.default, 'deny');
  assert.deepEqual(plan.project, { host: project, guest: '/work/sample project', mode: 'read-write' });
  const expectedHome = `${root}/state/homes/${sandbox}`;
  assert.equal(plan.home.host, expectedHome);
  assert.equal(plan.home.guest, '/root');
  check('Isolated plan and project mount', plan);

  const untrusted = await run(runner, ['brig', 'run', '-d', '--image', 'docker.io/library/ubuntu:24.04', ...runArguments]);
  assert.equal(untrusted.status, 5);
  assert.match(untrusted.stderr, /refusing to boot image.*BRIG_VERIFY=require/);
  assert.deepEqual((await json(['ls'], 'SandboxList')).sandboxes, []);
  check('Untrusted image refused without a boot', untrusted.status);

  const bootId = await boot();
  check('Signed image and boot assets accepted', { image: plan.image.ref, bootId, version });
  for (const [name, expected] of [[archive, archiveDigest], [`${archive}.sigstore`, bundleDigest]]) {
    const destination = path.join(root, 'downloads', name);
    await checked('/usr/bin/curl', ['-qfsSL', '--proto', '=https', '--connect-timeout', '10', '--max-time', '600', '--output', destination, `${releaseUrl}/${name}`], 610_000);
    const digest = createHash('sha256');
    for await (const bytes of fs.createReadStream(destination)) digest.update(bytes);
    assert.equal(digest.digest('hex'), expected);
  }
  const downloaded = `${root}/downloads/${archive}`;
  await checked(runner, ['cosign', 'verify-blob', '--bundle', `${downloaded}.sigstore`,
    '--certificate-identity', `https://github.com/openai/codex/.github/workflows/rust-release.yml@refs/tags/${release}`,
    '--certificate-oidc-issuer', 'https://token.actions.githubusercontent.com', downloaded]);
  const installation = path.join(expectedHome, '.local/share/tugboat-phase1/codex');
  fs.mkdirSync(installation, { recursive: true, mode: 0o700 });
  await checked('/usr/bin/tar', ['-xzf', downloaded, '-C', installation], 120_000);
  const localDigest = (await checked('/usr/bin/shasum', ['-a', '256', `${installation}/bin/codex`])).stdout.split(' ')[0];
  assert.equal((await guest('sha256sum', codex)).stdout.split(' ')[0], localDigest);
  const codexVersion = (await guest(codex, '--version')).stdout.trim();
  const claudeVersion = (await guest('claude', '--version')).stdout.trim();
  assert.equal(codexVersion, 'codex-cli 0.171.0');
  assert.match(claudeVersion, /^\d+\.\d+\.\d+ \(Claude Code\)$/);
  check('Both verified executables run on one guest', { codexVersion, claudeVersion, codexDigest: localDigest });

  const layout = await guest('sh', '-ec', 'test "$PWD" = "$BRIG_PROJECT"; uname -sm; findmnt -n -o SOURCE,FSTYPE -T "$BRIG_PROJECT"; stat -f -c %T "$CLAUDE_CONFIG_DIR"; test ! -e "$1"', 'phase1', evidence);
  assert.equal(layout.stdout.trim(), 'Linux aarch64\nshare1 virtiofs\ntmpfs');
  check('Guest mount and private host boundary', layout.stdout.trim());

  await guest(codex, 'sandbox', '-c', 'sandbox_mode="workspace-write"', '-c', 'sandbox_workspace_write.network_access=false', '--', '/bin/sh', '-ec',
    'printf workspace > from-workspace.txt; if (printf outside > /root/phase1-outside-workspace) 2>/dev/null; then exit 1; fi');
  assert.equal(fs.readFileSync(path.join(project, 'from-workspace.txt'), 'utf8'), 'workspace');
  await guest(codex, 'sandbox', '-c', 'sandbox_mode="danger-full-access"', '--', '/bin/sh', '-ec',
    'printf guest > /root/phase1-brig-boundary; test ! -e "$1"', 'phase1', evidence);
  const help = await guest('claude', '--help');
  assert.match(help.stdout, /--permission-mode[^]*?"manual"/);
  check('Codex sandbox modes and the Claude manual permission option', [codexVersion, claudeVersion]);

  const curlOptions = ['-q', '--noproxy', '*', '--connect-timeout', '8', '--max-time', '12', '-sS', '-o', '/dev/null', '-w', '%{http_code}'];
  const providerCodes = {};
  for (const host of ['chatgpt.com', 'api.anthropic.com']) {
    const code = (await guest('curl', ...curlOptions, `https://${host}`)).stdout.trim();
    assert.match(code, /^[1-5][0-9]{2}$/);
    providerCodes[host] = code;
  }
  await checked('/usr/bin/curl', [...curlOptions, 'https://example.com']);
  const denied = await run(runner, ['brig', 'exec', reference, '--', 'curl', ...curlOptions, 'https://example.com']);
  assert.equal(denied.status, 6);
  assert.match((await brig(['logs', '--gateway', reference, '--tail', '40'])).stdout, /egress: refused the [A-Z]+ query example\.com/);
  check('Isolated gateway allows provider endpoints and denies the control', providerCodes);

  const credentialScript = fs.readFileSync(new URL('../../src-tauri/src/providers/shared_claude/guest_credentials.py', import.meta.url), 'utf8');
  const input = JSON.stringify({ claudeAiOauth: { accessToken: 'phase1-synthetic', refreshToken: null } });
  const result = spawnSync(runner, ['brig', 'exec', reference, '--', 'python3', '-c', credentialScript, 'write'], { input, timeout: 25_000 });
  assert.equal(result.status, 0, 'The guest did not accept its synthetic access copy');
  await guest('sh', '-ec', 'test "$(stat -c %a "$CLAUDE_CONFIG_DIR/.credentials.json")" = 600; test ! -e "$HOME/.claude/.credentials.json"; printf retained > "$BRIG_PROJECT/restart.txt"');
  assert.equal((await guest('cat', '/proc/sys/kernel/random/boot_id')).stdout.trim(), bootId);
  await brig(['stop', reference]);
  assert.notEqual(await boot(), bootId);
  await guest('sh', '-ec', 'test ! -e "$CLAUDE_CONFIG_DIR/.credentials.json"; test "$(cat "$BRIG_PROJECT/restart.txt")" = retained');
  assert.equal((await json(['info', reference], 'Info')).workspace, expectedHome);
  check('Restart kept the project and cleared the Claude access copy', reference);
  report.result = 'infrastructure-passed';
} catch (error) {
  report.result = 'failed';
  report.error = error.message;
  process.exitCode = 1;
} finally {
  if (!keep || process.exitCode) {
    try {
      report.detachAttempts = await cleanSharedProject(
        reference,
        async () => (await json(['ls'], 'SandboxList')).sandboxes,
        brig,
        () => run(runner, ['hull', 'store', 'detach']),
      );
    } catch (error) {
      report.cleanupError = error.message;
      process.exitCode = 1;
    }
  }
  save();
  console.log(`${report.result}. Evidence ${evidence}`);
}
