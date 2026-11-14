import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runAcceptanceProcess } from './acceptance-process.mjs';
import { runtimeConfigurationFile } from './acceptance-runtime.mjs';

const workspace = fileURLToPath(new URL('../../', import.meta.url));
const root = process.argv[2] ?? fs.mkdtempSync('/private/tmp/tga-');
const brigPath = process.argv[3] ?? path.join(workspace, '.build/brig-runtime/bin/brig');
assert.ok(root.startsWith('/private/tmp/tga-') && fs.realpathSync(root) === root);
assert.equal(fs.statSync(root).uid, process.getuid());
assert.deepEqual(fs.readdirSync(root), [], 'Use a new, empty test directory.');
fs.chmodSync(root, 0o700);
const project = path.join(root, 'sample project');
const runtime = process.argv[4] ?? path.join(workspace, '.build/brig-runtime/bin/hull');
const store = path.join(root, 'h');
const runtimeWrapper = path.join(root, 'bin/hull');
const runtimeStores = path.join(root, 'runtime-stores');
const evidence = path.join(workspace, 'artifacts/claude-code/step-9-cli-environments.json');
for (const name of ['sample project', 'state', 'policies', 'profiles', 'config', 'cache', 'data', 'xdg-state', 'docker', 'tmp', 'bin']) fs.mkdirSync(path.join(root, name));
fs.writeFileSync(path.join(root, 'docker/config.json'), '{}\n', { mode: 0o600 });
fs.copyFileSync(path.join(workspace, 'src-tauri/profiles/tugboat-claude.yaml'), path.join(root, 'profiles/tugboat-claude.yaml'));
fs.writeFileSync(path.join(project, 'README.md'), 'Temporary dual-agent acceptance project.\n');
const inherited = Object.fromEntries(['HOME', 'USER', 'LOGNAME'].filter(name => process.env[name] !== undefined).map(name => [name, process.env[name]]));
const environment = {
  ...inherited,
  PATH: [path.dirname(runtimeWrapper), path.dirname(brigPath), path.dirname(runtime), '/usr/bin', '/bin', '/usr/sbin', '/sbin'].join(':'),
  LANG: 'en_US.UTF-8', TERM: 'dumb', TMPDIR: root + '/tmp/',
  XDG_CONFIG_HOME: root + '/config', XDG_CACHE_HOME: root + '/cache',
  DOCKER_CONFIG: root + '/docker',
  XDG_DATA_HOME: root + '/data', XDG_STATE_HOME: root + '/xdg-state', BRIG_STATE_DIR: root + '/state',
  BRIG_POLICY_DIR: root + '/policies', BRIG_PROFILE_DIR: root + '/profiles',
  BRIG_HYPERVISOR: 'hvi', BRIG_RUNTIME: 'hull', BRIG_RUNTIME_BIN: runtimeWrapper,
  BRIG_NETWORK: 'isolated', BRIG_VERIFY: 'require', DO_NOT_TRACK: '1',
  MKE2FS_CONFIG: runtimeConfigurationFile(runtime),
  BRIG_SKILLS: '0', BRIG_GIT_CONFIG: '0', BRIG_FORWARD_ENV: '',
};
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
fs.writeFileSync(runtimeWrapper, `#!/bin/sh\nprintf '%s\\n' ${quote(store)} >> ${quote(runtimeStores)}\nexec ${quote(runtime)} --store-dir ${quote(store)} "$@"\n`, { mode: 0o700 });
const agents = [
  { kind: 'codex', profile: 'tugboat-test-codex', label: 'co', hosts: ['auth.openai.com', 'chatgpt.com', 'api.openai.com'] },
  { kind: 'claude-code', profile: 'tugboat-claude', label: 'cl', hosts: ['api.anthropic.com', 'platform.claude.com', 'claude.ai'] },
].map(agent => ({ ...agent, reference: agent.profile + '@' + agent.label, policy: 'accept-' + agent.label, homePrefix: path.join(root, 'guest-' + agent.kind) }));
const report = {
  schemaVersion: 1, startedAt: new Date().toISOString(), root, project,
  isolatedHome: false, isolatedGuestHomes: true, hostHomePreserved: true, store, runtimeWrapper,
  userInterfaceTest: false, accountTest: false, modelRequestTest: false,
  agents, commands: [], checks: [], outcome: 'running',
};
fs.mkdirSync(path.dirname(evidence), { recursive: true });
const save = () => fs.writeFileSync(evidence, JSON.stringify(report, null, 2) + '\n');
async function command(binary, args, timeout = 30_000) {
  const result = await runAcceptanceProcess(binary, args, { env: environment, cwd: project, timeout });
  const record = { binary, args, timeoutMs: timeout, exitCode: result.status, error: result.error, signal: result.signal, terminationError: result.terminationError ?? null, stdout: result.stdout, stderr: result.stderr };
  report.commands.push(record); save();
  assert.equal(record.error, null, 'The acceptance command did not finish.');
  assert.equal(record.signal, null, 'The acceptance command was interrupted.');
  assert.equal(record.exitCode, 0, record.stderr || 'The acceptance command failed.');
  return record.stdout;
}
const brig = (args, timeout) => command(brigPath, args, timeout);
async function json(args, kind) {
  const value = JSON.parse(await brig(['--json', ...args]));
  assert.equal(value.apiVersion, 'brig.sh/v1alpha1');
  assert.equal(value.kind, kind);
  return value.data;
}
async function check(name, verify) {
  await verify(); report.checks.push({ name, passed: true }); save();
}
async function remove(agent) {
  await brig(['stop', agent.reference]);
  assert.ok((await brig(['rm', agent.reference, '--dry-run'])).includes(agent.reference), 'Removal preview does not identify the selected environment.');
  await brig(['rm', agent.reference]);
}
const policySource = path.join(root, 'policy.yaml');
const editor = path.join(root, 'editor');
fs.writeFileSync(editor, '#!/bin/sh\nexec /bin/cp "$TUGBOAT_AGENT_POLICY" "$1"\n', { mode: 0o700 });
Object.assign(environment, { EDITOR: editor, VISUAL: editor, TUGBOAT_AGENT_POLICY: policySource });
const inventory = async () => (await json(['ls'], 'SandboxList')).sandboxes;
const storeCalls = () => fs.existsSync(runtimeStores) ? fs.readFileSync(runtimeStores, 'utf8').split('\n').filter(Boolean) : [];
const passed = 'passed-cli-with-native-and-account-acceptance-pending';
save();
try {
  assert.ok(fs.existsSync(runtime), 'Prepare the local hull runtime first.');
  await json(['version'], 'Version');
  assert.deepEqual(await inventory(), [], 'The test state must have no existing environments.');
  const codexProfile = JSON.parse(await brig(['agent', 'show', 'codex', '--json']));
  assert.equal(codexProfile.name, 'codex'); assert.equal(codexProfile.binary, 'codex');
  codexProfile.name = agents[0].profile;
  codexProfile.secrets = []; codexProfile.files = [];
  codexProfile.env = (codexProfile.env ?? []).filter(binding => !binding.ref && !/TOKEN|KEY|SECRET|CREDENTIAL|AUTH/i.test(binding.name));
  fs.writeFileSync(path.join(root, 'profiles', codexProfile.name + '.yaml'), JSON.stringify(codexProfile, null, 2) + '\n');
  report.codexProfile = { name: codexProfile.name, derivedFrom: 'codex', credentialSourcesRemoved: true };
  for (const agent of agents) {
    fs.writeFileSync(policySource, `apiVersion: brig.sh/v1alpha1\nkind: Policy\nmetadata:\n  name: ${agent.policy}\nspec:\n  egress:\n    default: deny\n    allow:\n${agent.hosts.map(host => '      - host: ' + host).join('\n')}\n`);
    await brig(['policy', 'create', agent.policy]);
    await brig(['policy', 'attach', agent.policy, agent.profile, '-n', agent.label]);
    agent.arguments = ['--network', 'isolated', '--home', agent.homePrefix, '--cpus', '2', '--mem', '2048', agent.reference, project];
    const plan = await json(['plan', ...agent.arguments], 'Plan');
    await check(agent.kind + '-plan-target', () => {
      assert.equal(plan.profile, agent.profile); assert.equal(plan.session, agent.label);
      assert.equal(plan.project.host, project); assert.equal(plan.project.guest, '/work/sample project');
      assert.equal(plan.home.host, agent.homePrefix + '-' + agent.label);
      assert.equal(plan.runtime.kind, 'hull'); assert.equal(plan.runtime.backend, 'hvi');
      assert.equal(plan.runtime.bin, runtimeWrapper, 'The plan must use the isolated hull wrapper.');
      assert.equal(plan.runtime.available, true); assert.equal(plan.refused, undefined);
      assert.equal(plan.verify.mode, 'require'); assert.equal(plan.noPolicy, false);
      assert.equal(plan.egress.default, 'deny'); assert.deepEqual(plan.policies, [agent.policy]);
      assert.deepEqual(plan.egress.allow.map(rule => rule.host).sort(), agent.hosts.toSorted());
      assert.deepEqual(plan.credentials, []);
    });
    agent.home = plan.home.host;
    const callsBefore = storeCalls().length;
    const version = await brig(['run', ...agent.arguments, '--', '--version'], 900_000);
    assert.match(version, agent.kind === 'codex' ? /codex/i : /claude/i);
    const info = await json(['info', agent.reference], 'Info');
    assert.equal(info.project.host, project); assert.equal(info.workspace, agent.home);
    agent.markerGuest = agent.kind === 'codex' ? '/root/.codex/acceptance-marker' : '/brig/claude/projects/acceptance-marker';
    agent.markerHost = path.join(agent.home, agent.kind === 'codex' ? '.codex/acceptance-marker' : '.claude/projects/acceptance-marker');
    await brig(['sh', agent.reference, 'mkdir', '-p', path.posix.dirname(agent.markerGuest)]);
    await brig(['sh', agent.reference, 'sh', '-c', `printf '${agent.kind}' > '${agent.markerGuest}'`]);
    agent.storeCalls = storeCalls().length - callsBefore;
    assert.ok(agent.storeCalls > 0, 'The guest did not use the test hull wrapper.');
    assert.ok(storeCalls().every(value => value === store), 'A guest used another hull store.');
    await check(agent.kind + '-persistent-home', () => assert.equal(fs.readFileSync(agent.markerHost, 'utf8'), agent.kind));
  }
  const [codex, claude] = agents;
  assert.notEqual(codex.home, claude.home);
  const policyHash = createHash('sha256').update(fs.readFileSync(path.join(root, 'policies', codex.policy + '.yaml'))).digest('hex');
  await brig(['stop', claude.reference]);
  await check('claude-stop-preserves-codex-guest', async () => assert.equal((await brig(['sh', codex.reference, 'cat', codex.markerGuest])).trim(), 'codex'));
  await remove(claude);
  await check('claude-removal-preserves-codex-home-policy-and-guest', async () => {
    assert.equal((await brig(['sh', codex.reference, 'cat', codex.markerGuest])).trim(), 'codex');
    assert.equal(fs.readFileSync(codex.markerHost, 'utf8'), 'codex');
    assert.equal(createHash('sha256').update(fs.readFileSync(path.join(root, 'policies', codex.policy + '.yaml'))).digest('hex'), policyHash);
  });
  report.outcome = passed;
} catch (error) {
  report.outcome = 'blocked'; report.blocker = error.message;
} finally {
  const errors = [];
  for (const agent of agents) {
    try {
      if ((await inventory()).some(entry => entry.ref === agent.reference)) await remove(agent);
    } catch (error) { errors.push('Could not remove ' + agent.reference + ': ' + error.message); }
  }
  try {
    report.remaining = await inventory();
    if (report.remaining.length) errors.push('Temporary environments remain.');
  } catch (error) { errors.push('Could not verify final inventory: ' + error.message); }
  try {
    for (let attempt = 0; attempt < 3; attempt++) {
      report.detachAttempts = attempt + 1;
      try { await command(runtime, ['--store-dir', store, 'store', 'detach']); break; }
      catch (error) {
        const last = report.commands.at(-1);
        if (attempt === 2 || last.error !== null || last.signal !== null || !/resource busy|device busy/i.test(last.stderr)) throw error;
        await new Promise(resolve => setTimeout(resolve, 500 * (attempt + 1)));
      }
    }
  }
  catch (error) { errors.push('Could not detach the isolated hull store: ' + error.message); }
  if (!errors.length) {
    report.runtimeStores = storeCalls();
    try { fs.rmSync(root, { recursive: true }); report.testRootRemoved = true; }
    catch (error) { errors.push('Could not remove the test directory: ' + error.message); }
  }
  if (errors.length) { report.outcome = 'blocked'; report.cleanupError = errors.join('; '); }
  report.completedAt = new Date().toISOString(); save();
  process.exitCode = report.outcome === passed ? 0 : 1;
  console.log('Outcome:', report.outcome, 'Evidence:', evidence);
}
