import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { runAcceptanceProcess } from './acceptance-process.mjs';

const workspace = fileURLToPath(new URL('../../', import.meta.url));
const requiredTools = ['brig', 'brigd', 'brig-broker', 'hull', 'hvi', 'vz-runner', 'cosign', 'mke2fs', 'debugfs', 'e2fsck'];
const homebrewPrefixes = ['/opt/homebrew', '/usr/local'];
const homebrewExecutables = homebrewPrefixes.flatMap(prefix => [path.join(prefix, 'bin/brew'), path.join(prefix, 'Homebrew/bin/brew')]);
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
const digest = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');

export const homebrewDeniedProfile = '(version 1)\n(allow default)\n(deny file-read* (subpath "/opt/homebrew") (subpath "/usr/local"))\n(deny process-exec (subpath "/opt/homebrew") (subpath "/usr/local"))\n';

export async function checkBundledRuntime(application, requestedRoot, { cleanMac = false, cachedStore = null } = {}) {
  assert.equal(process.platform, 'darwin', 'The runtime acceptance needs macOS.');
  assert.equal(process.arch, 'arm64', 'The runtime acceptance needs Apple silicon.');
  assert.ok(path.isAbsolute(application), 'Pass the absolute path to Tugboat.app.');
  const app = fs.realpathSync(application);
  const bin = path.join(app, 'Contents/MacOS');
  const manifest = path.join(app, 'Contents/Resources/runtime/build.json');
  for (const name of ['tugboat', ...requiredTools]) {
    fs.accessSync(path.join(bin, name), fs.constants.X_OK);
  }
  fs.accessSync(manifest, fs.constants.R_OK);
  const installedHomebrew = homebrewExecutables.filter(file => fs.existsSync(file));
  if (cleanMac) assert.deepEqual(installedHomebrew, [], 'Clean Mac acceptance requires Homebrew to be absent.');
  const macOSVersion = execFileSync('/usr/bin/sw_vers', ['-productVersion'], { encoding: 'utf8', timeout: 5_000 }).trim();
  const manifestSha256 = digest(manifest);
  if (cachedStore) {
    assert.match(cachedStore, /^\/private\/tmp\/tgb-[^/]+\/h\.sparseimage$/);
    assert.equal(fs.realpathSync(cachedStore), cachedStore);
    const metadata = fs.lstatSync(cachedStore);
    assert.ok(metadata.isFile() && metadata.uid === process.getuid(), 'Use an owned regular cache snapshot.');
    const cacheRoot = path.dirname(cachedStore);
    const sourceReport = JSON.parse(fs.readFileSync(path.join(workspace, 'artifacts/bundled-runtime', path.basename(cacheRoot) + '.json'), 'utf8'));
    assert.equal(sourceReport.root, cacheRoot);
    assert.equal(sourceReport.manifestSha256, manifestSha256, 'The cache must use the same runtime bundle.');
    assert.ok(sourceReport.completedAt && !sourceReport.cleanupError, 'Finish cache cleanup before retrying.');
    assert.deepEqual(sourceReport.remaining, [], 'The source cache must have no remaining guests.');
    const detached = sourceReport.commands.findLast(record => path.basename(record.binary) === 'hull' && JSON.stringify(record.args) === JSON.stringify(['--store-dir', cacheRoot + '/h', 'store', 'detach']));
    assert.ok(detached?.exitCode === 0 && detached.error === null && detached.signal === null, 'The source cache must be detached.');
  }
  const root = requestedRoot ?? fs.mkdtempSync('/private/tmp/tgb-');
  assert.ok(root.startsWith('/private/tmp/tgb-') && fs.realpathSync(root) === root, 'Use an isolated /private/tmp/tgb- directory.');
  assert.equal(fs.statSync(root).uid, process.getuid());
  assert.deepEqual(fs.readdirSync(root), [], 'Use a new empty acceptance directory.');
  fs.chmodSync(root, 0o700);
  const evidence = path.join(workspace, 'artifacts/bundled-runtime', path.basename(root) + '.json');
  const project = path.join(root, 'örnek proje');
  const store = path.join(root, 'h');
  for (const directory of ['home', 'state', 'policies', 'profiles', 'config', 'cache', 'data', 'xdg-state', 'docker', 'tmp', 'bin', 'örnek proje']) {
    fs.mkdirSync(path.join(root, directory));
  }
  fs.writeFileSync(path.join(root, 'docker/config.json'), '{}\n', { mode: 0o600 });
  fs.writeFileSync(path.join(project, 'host marker.txt'), 'bundled-runtime-host\n');
  const offlineSandbox = path.join(root, 'offline.sb');
  fs.writeFileSync(offlineSandbox, homebrewDeniedProfile + '(deny network*)\n');
  const wrapper = path.join(root, 'bin/hull');
  const storeCalls = path.join(root, 'store-calls');
  fs.writeFileSync(wrapper, '#!/bin/sh\nprintf \'%s\\n\' ' + quote(store) + ' >> ' + quote(storeCalls) + '\nexec ' + quote(path.join(bin, 'hull')) + ' --store-dir ' + quote(store) + ' "$@"\n', { mode: 0o700 });
  const environment = {
    HOME: path.join(root, 'home'), USER: os.userInfo().username, LOGNAME: os.userInfo().username,
    PATH: bin + ':/usr/bin:/bin:/usr/sbin:/sbin', LANG: 'en_US.UTF-8', TERM: 'dumb', TMPDIR: root + '/tmp/',
    XDG_CONFIG_HOME: root + '/config', XDG_CACHE_HOME: root + '/cache', XDG_DATA_HOME: root + '/data', XDG_STATE_HOME: root + '/xdg-state',
    DOCKER_CONFIG: root + '/docker', BRIG_STATE_DIR: root + '/state', BRIG_POLICY_DIR: root + '/policies', BRIG_PROFILE_DIR: root + '/profiles',
    MKE2FS_CONFIG: path.join(app, 'Contents/Resources/runtime/mke2fs.conf'),
    BRIG_HYPERVISOR: 'hvi', BRIG_RUNTIME: 'hull', BRIG_RUNTIME_BIN: wrapper, BRIG_NETWORK: 'isolated', BRIG_VERIFY: 'require', HULL_VERIFY: 'require',
    DO_NOT_TRACK: '1', BRIG_SKILLS: '0', BRIG_GIT_CONFIG: '0', BRIG_FORWARD_ENV: '',
  };
  const policySource = path.join(root, 'policy.yaml');
  const editor = path.join(root, 'editor');
  fs.writeFileSync(editor, '#!/bin/sh\nexec /bin/cp "$TUGBOAT_POLICY_SOURCE" "$1"\n', { mode: 0o700 });
  Object.assign(environment, { EDITOR: editor, VISUAL: editor, TUGBOAT_POLICY_SOURCE: policySource });
  const agents = [
    { kind: 'codex', source: 'codex', profile: 'tgb-codex', label: 'co', hosts: ['api.openai.com', 'auth.openai.com', 'chatgpt.com'] },
    { kind: 'claude-code', source: 'claude-code', profile: 'tgb-claude', label: 'cl', hosts: ['api.anthropic.com', 'claude.ai', 'platform.claude.com'] },
  ].map(agent => ({ ...agent, reference: agent.profile + '@' + agent.label, policy: 'tgb-' + agent.label, homePrefix: path.join(root, 'g' + agent.label) }));
  const report = {
    schemaVersion: 1, startedAt: new Date().toISOString(), application: app, root, project, store,
    host: { os: os.release(), macOSVersion, architecture: process.arch }, isolatedHome: true, coldGuestCache: !cachedStore,
    cacheSource: cachedStore,
    homebrewInstalledOnHost: installedHomebrew.length > 0,
    homebrewAccessBlocked: false, preflightHomebrewAccessBlocked: false, separateCleanMacTest: cleanMac, minimumMacOS15Test: macOSVersion.split('.')[0] === '15',
    cleanMacContext: cleanMac ? { source: 'user-provided', homebrewAbsentAtStandardPaths: true } : null,
    userInterfaceTest: false, accountTest: false, modelRequestTest: false,
    manifestSha256, tools: Object.fromEntries(requiredTools.map(name => [name, { path: path.join(bin, name), sha256: digest(path.join(bin, name)) }])),
    agents, commands: [], checks: [], outcome: 'running',
  };
  fs.mkdirSync(path.dirname(evidence), { recursive: true });
  const save = () => fs.writeFileSync(evidence, JSON.stringify(report, null, 2) + '\n');
  async function command(binary, args, { timeout = 30_000, offline = false, checked = true } = {}) {
    console.log('Running ' + path.basename(binary) + ' ' + args.slice(0, 2).join(' '));
    const program = offline ? '/usr/bin/sandbox-exec' : binary;
    const argumentsList = offline ? ['-f', offlineSandbox, binary, ...args] : args;
    const result = await runAcceptanceProcess(program, argumentsList, { env: environment, cwd: project, timeout });
    const record = { binary, args, offline, preflightSandbox: offline, timeoutMs: timeout, exitCode: result.status, error: result.error, signal: result.signal, terminationError: result.terminationError ?? null, stdout: result.stdout, stderr: result.stderr };
    report.commands.push(record); save();
    assert.equal(record.error, null, record.error || record.stderr);
    assert.equal(record.signal, null, record.stderr || 'The command was interrupted.');
    if (checked) assert.equal(record.exitCode, 0, record.stderr || record.stdout);
    return record;
  }
  const brig = (args, options) => command(path.join(bin, 'brig'), args, options);
  async function json(args, kind) {
    const value = JSON.parse((await brig(['--json', ...args])).stdout);
    assert.equal(value.apiVersion, 'brig.sh/v1alpha1'); assert.equal(value.kind, kind);
    return value.data;
  }
  const inventory = async () => (await json(['ls'], 'SandboxList')).sandboxes;
  async function check(name, operation) {
    await operation(); report.checks.push({ name, passed: true }); save();
    console.log('Passed ' + name);
  }
  async function remove(agent) {
    assert.ok(agent.reference.startsWith('tgb-') && agent.home?.startsWith(root + '/'), 'Cleanup must target this test.');
    await brig(['stop', agent.reference]);
    const preview = await brig(['rm', agent.reference, '--dry-run']);
    assert.ok(preview.stdout.includes(agent.reference));
    await brig(['rm', agent.reference]);
  }
  let startedGuest = false;
  const passed = cleanMac ? 'passed-on-clean-mac' : 'passed-bundled-runtime';
  save();
  try {
    if (cachedStore) await check('detached-cache-clone', () => command('/bin/cp', ['-c', cachedStore, path.join(root, 'h.sparseimage')], { timeout: 120_000 }));
    await check('homebrew-prefixes-denied', async () => {
      for (const prefix of homebrewPrefixes) {
        if (!fs.existsSync(prefix)) continue;
        const result = await command('/bin/cat', [prefix], { checked: false, offline: true });
        assert.notEqual(result.exitCode, 0);
        assert.match(result.stderr, /Operation not permitted|Permission denied/);
        if (!fs.existsSync(prefix + '/bin/brew')) continue;
        const execution = await command('/bin/sh', ['-c', 'exec "$1" --version', 'brew-execution-probe', prefix + '/bin/brew'], { checked: false, offline: true });
        assert.notEqual(execution.exitCode, 0);
        assert.match(execution.stderr, /Operation not permitted|Permission denied/);
      }
      report.preflightHomebrewAccessBlocked = true;
    });
    await check('packaged-app-offline-runtime-check', async () => {
      const result = await command(path.join(bin, 'tugboat'), ['--runtime-check'], { offline: true, timeout: 60_000 });
      const status = JSON.parse(result.stdout);
      assert.equal(status.brigSource, 'bundled'); assert.equal(status.brigStatus, 'installed');
      assert.equal(status.version.data.version, 'v0.5.2');
      assert.equal(status.tools.brig, path.join(bin, 'brig')); assert.equal(status.tools.hull, path.join(bin, 'hull'));
      report.runtimeCheck = status;
    });
    assert.deepEqual(await inventory(), []);
    for (const agent of agents) {
      const profile = JSON.parse((await brig(['agent', 'show', agent.source, '--json'])).stdout);
      profile.name = agent.profile; profile.secrets = []; profile.files = [];
      profile.env = (profile.env ?? []).filter(binding => !binding.ref && !/TOKEN|KEY|SECRET|CREDENTIAL|AUTH/i.test(binding.name));
      fs.writeFileSync(path.join(root, 'profiles', agent.profile + '.yaml'), JSON.stringify(profile, null, 2) + '\n');
      fs.writeFileSync(policySource, 'apiVersion: brig.sh/v1alpha1\nkind: Policy\nmetadata:\n  name: ' + agent.policy + '\nspec:\n  egress:\n    default: deny\n    allow:\n' + agent.hosts.map(host => '      - host: ' + host).join('\n') + '\n');
      await brig(['policy', 'create', agent.policy]);
      await brig(['policy', 'attach', agent.policy, agent.profile, '-n', agent.label]);
      agent.arguments = ['--network', 'isolated', '--home', agent.homePrefix, '--cpus', '2', '--mem', '2048', agent.reference, project];
      const plan = await json(['plan', ...agent.arguments], 'Plan');
      await check(agent.kind + '-exact-plan', () => {
        assert.equal(plan.project.host, project); assert.equal(plan.project.guest, '/work/' + path.basename(project));
        assert.equal(plan.runtime.bin, wrapper); assert.equal(plan.runtime.backend, 'hvi'); assert.equal(plan.runtime.available, true);
        assert.match(plan.isolation, /\brootfs overlay-block\b/, 'The bundled tools must select the ext4 overlay root.');
        assert.equal(plan.verify.mode, 'require'); assert.equal(plan.refused, undefined); assert.equal(plan.noPolicy, false);
        assert.equal(plan.egress.default, 'deny'); assert.deepEqual(plan.policies, [agent.policy]); assert.deepEqual(plan.credentials, []);
        assert.equal(plan.home.host, agent.homePrefix + '-' + agent.label);
        agent.home = plan.home.host;
        agent.isolation = plan.isolation;
      });
      startedGuest = true;
      console.log('Starting ' + agent.kind + ' with the bundled runtime.');
      await check(agent.kind + '-cold-hvi-boot', async () => {
        const result = await brig(['run', ...agent.arguments, '--', '--version'], { timeout: 1_800_000 });
        assert.match(result.stdout, agent.kind === 'codex' ? /codex/i : /claude/i);
        const info = await json(['info', agent.reference], 'Info');
        assert.equal(info.project.host, project); assert.equal(info.workspace, agent.home);
        assert.equal((await brig(['sh', agent.reference, 'uname', '-s'])).stdout.trim(), 'Linux');
        const instance = JSON.parse((await command(path.join(bin, 'hull'), ['--store-dir', store, 'inspect', plan.sandbox])).stdout);
        assert.equal(instance.backend, 'hvi');
        assert.equal(instance.cmdLine.includes('--no-sandbox'), false, 'The hvi guest must retain its own sandbox.');
        const disks = instance.cmdLine.filter((value, index) => instance.cmdLine[index - 1] === '--disk');
        assert.equal(disks.length, 2, 'The guest must use read-only lower and writable upper ext4 disks.');
        assert.match(disks[0], /\/rootfs\.ext4,readonly=on,serial=disk0$/);
        assert.match(disks[1], /\/rootfs-upper\.ext4,serial=disk1$/);
        agent.rootDisks = [];
        for (const disk of disks) {
          const file = disk.slice('path='.length).split(',')[0];
          const descriptor = fs.openSync(file, 'r');
          const signature = Buffer.alloc(2);
          try { assert.equal(fs.readSync(descriptor, signature, 0, 2, 1080), 2); }
          finally { fs.closeSync(descriptor); }
          assert.equal(signature.toString('hex'), '53ef', 'The root disk must contain an ext4 filesystem.');
          const metadata = fs.statSync(file);
          agent.rootDisks.push({ path: file, bytes: metadata.size, allocatedBytes: metadata.blocks * 512 });
        }
      });
      agent.marker = '/root/.tgb-marker';
      const sharedFiles = await brig(['sh', agent.reference, 'sh', '-c', 'set -eu\nprintf ' + quote(agent.kind) + ' > ' + quote(agent.marker) + '\ncat "$BRIG_PROJECT/host marker.txt"\nprintf ' + quote(agent.kind) + ' > "$BRIG_PROJECT/' + agent.label + '-guest.txt"']);
      assert.equal(sharedFiles.stdout.trim(), 'bundled-runtime-host');
      assert.equal(fs.readFileSync(path.join(project, agent.label + '-guest.txt'), 'utf8'), agent.kind);
      await check(agent.kind + '-stop-restart-preserves-home', async () => {
        await brig(['stop', agent.reference]);
        await brig(['run', ...agent.arguments, '--', '--version'], { timeout: 120_000 });
        assert.equal((await brig(['sh', agent.reference, 'cat', agent.marker])).stdout.trim(), agent.kind);
        assert.equal((await json(['info', agent.reference], 'Info')).workspace, agent.home);
      });
    }
    await check('separate-agent-environments', async () => {
      assert.notEqual(agents[0].home, agents[1].home);
      await remove(agents[1]);
      assert.equal((await brig(['sh', agents[0].reference, 'cat', agents[0].marker])).stdout.trim(), 'codex');
    });
    report.runtimeStores = fs.readFileSync(storeCalls, 'utf8').split('\n').filter(Boolean);
    assert.ok(report.runtimeStores.length > 0 && report.runtimeStores.every(value => value === store));
    report.outcome = passed;
  } catch (error) {
    report.outcome = 'failed'; report.failure = error.message;
  } finally {
    const failures = [];
    if (startedGuest) {
      for (const agent of agents) {
        try { if ((await inventory()).some(value => value.ref === agent.reference)) await remove(agent); }
        catch (error) { failures.push('Could not remove ' + agent.reference + ': ' + error.message); }
      }
      try { report.remaining = await inventory(); assert.deepEqual(report.remaining, []); }
      catch (error) { failures.push('Final inventory: ' + error.message); }
      try {
        for (let attempt = 1; attempt <= 3; attempt++) {
          report.detachAttempts = attempt;
          const result = await command(path.join(bin, 'hull'), ['--store-dir', store, 'store', 'detach'], { checked: false });
          if (result.exitCode === 0) break;
          assert.ok(attempt < 3 && /resource busy|device busy/i.test(result.stderr), result.stderr);
          await new Promise(resolve => setTimeout(resolve, 500 * attempt));
        }
      } catch (error) { failures.push('Store detach: ' + error.message); }
    }
    if (failures.length) { report.outcome = 'failed'; report.cleanupError = failures.join('; '); }
    if (report.outcome === passed) {
      try { fs.rmSync(root, { recursive: true }); report.testRootRemoved = true; }
      catch (error) { report.outcome = 'failed'; report.cleanupError = error.message; }
    }
    report.completedAt = new Date().toISOString(); save();
  }
  return { evidence, report };
}

if (process.argv[1] && import.meta.url === pathToFileURL(fs.realpathSync(process.argv[1])).href) {
  const argumentsList = process.argv.slice(2);
  const cleanMac = argumentsList.includes('--clean-mac');
  const [application, root, extra] = argumentsList.filter(value => value !== '--clean-mac');
  assert.ok(application && !extra, 'Usage: node scripts/acceptance/check-bundled-runtime.mjs /absolute/path/Tugboat.app [new /private/tmp/tgb- directory] [--clean-mac]');
  const { evidence, report } = await checkBundledRuntime(application, root, { cleanMac });
  process.exitCode = ['passed-bundled-runtime', 'passed-on-clean-mac'].includes(report.outcome) ? 0 : 1;
  console.log(JSON.stringify({ outcome: report.outcome, checks: report.checks.length, evidence, failure: report.failure, cleanupError: report.cleanupError }));
}
