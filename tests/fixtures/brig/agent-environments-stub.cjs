const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { syncBuiltinESMExports } = require('node:module');
const { EventEmitter } = require('node:events');
const { PassThrough } = require('node:stream');
const stage = process.env.TEST_BRIG_STAGE;
const guests = new Map();
let detachAttempts = 0;
const answer = (stdout = '', status = 0, stderr = '') => ({ status, signal: null, stdout, stderr });
const envelope = (kind, data) => answer(JSON.stringify({ apiVersion: 'brig.sh/v1alpha1', kind, data }));
const removeDirectory = fs.rmSync;
if (stage === 'root-cleanup') fs.rmSync = (target, options) => {
  if (target.startsWith('/private/tmp/tga-')) throw new Error('test directory unavailable');
  return removeDirectory(target, options);
};
function fixtureCommand(binary, args, { env, cwd }) {
  assert.ok(path.dirname(binary).endsWith('/.build/brig-runtime/bin'));
  assert.equal(env.PATH.includes('/opt/homebrew'), false);
  assert.equal(env.PATH.includes('/usr/local'), false);
  assert.equal(env.HOME, process.env.HOME);
  for (const field of ['ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN', 'CLAUDE_CODE_OAUTH_TOKEN', 'OPENAI_API_KEY', 'GH_TOKEN']) assert.equal(env[field], undefined);
  assert.ok(env.BRIG_PROFILE_DIR.startsWith('/private/tmp/tga-'));
  assert.equal(env.XDG_CACHE_HOME, path.join(path.dirname(cwd), 'cache'));
  assert.equal(env.XDG_DATA_HOME, path.join(path.dirname(cwd), 'data'));
  assert.equal(env.XDG_STATE_HOME, path.join(path.dirname(cwd), 'xdg-state'));
  assert.equal(env.DOCKER_CONFIG, path.join(path.dirname(cwd), 'docker'));
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(env.DOCKER_CONFIG, 'config.json'), 'utf8')), {});
  assert.equal(env.BRIG_SKILLS, '0'); assert.equal(env.BRIG_GIT_CONFIG, '0');
  assert.equal(env.BRIG_FORWARD_ENV, '');
  if (path.basename(binary) === 'hull') {
    assert.deepEqual(args, ['--store-dir', path.join(path.dirname(cwd), 'h'), 'store', 'detach']);
    detachAttempts++;
    if (stage === 'detach-exhausted' || (stage === 'detach-busy' && detachAttempts === 1)) return answer('', 16, 'Resource busy');
    return answer('', stage === 'detach' ? 2 : 0, stage === 'detach' ? 'detach unavailable' : '');
  }
  if (args[0] === 'agent') {
    assert.deepEqual(args, ['agent', 'show', 'codex', '--json']);
    return answer(JSON.stringify({
      name: 'codex', binary: 'codex', kind: 'agent', image: 'ghcr.io/brig-sh/codex-stock:root',
      command: ['codex'], volumes: [{ kind: 'hostmount', path: '.codex' }], initScripts: ['keep-default-boot'],
      secrets: [{ name: 'OPENAI_API_KEY' }], files: [{ path: '~/.codex/auth.json' }],
      env: [{ name: 'GH_TOKEN', ref: 'env.GH_TOKEN' }, { name: 'OPENAI_API_KEY', value: 'synthetic-host-value' }, { name: 'IS_SANDBOX', value: '1' }],
    }));
  }
  if (args[0] === '--json') {
    if (args[1] === 'version') return stage === 'version' ? answer('', 2, 'Brig fixture unavailable') : envelope('Version', { version: 'v0.5.2' });
    if (args[1] === 'ls') {
      if (stage === 'inventory' && guests.size === 0) return answer('', 2, 'inventory unavailable');
      return envelope('SandboxList', { sandboxes: [...guests].map(([ref, guest]) => ({ ref, state: guest.running ? 'running' : 'stopped' })) });
    }
    if (args[1] === 'plan') {
      const reference = args.at(-2);
      const [profile, label] = reference.split('@');
      const isClaude = profile === 'tugboat-claude';
      const home = args[args.indexOf('--home') + 1] + '-' + label;
      assert.equal(args.at(-1), cwd);
      assert.equal(env.BRIG_VERIFY, 'require');
      assert.deepEqual(args.slice(2, 10), ['--network', 'isolated', '--home', path.join(path.dirname(cwd), 'guest-' + (isClaude ? 'claude-code' : 'codex')), '--cpus', '2', '--mem', '2048']);
      const hosts = isClaude ? ['api.anthropic.com', 'platform.claude.com', 'claude.ai'] : ['auth.openai.com', 'chatgpt.com', 'api.openai.com'];
      assert.ok(fs.existsSync(path.join(env.BRIG_PROFILE_DIR, 'tugboat-claude.yaml')));
      const derived = JSON.parse(fs.readFileSync(path.join(env.BRIG_PROFILE_DIR, 'tugboat-test-codex.yaml'), 'utf8'));
      assert.equal(derived.name, 'tugboat-test-codex');
      assert.equal(derived.image, 'ghcr.io/brig-sh/codex-stock:root');
      assert.deepEqual(derived.secrets, []); assert.deepEqual(derived.files, []);
      assert.deepEqual(derived.env, [{ name: 'IS_SANDBOX', value: '1' }]);
      assert.deepEqual(derived.command, ['codex']);
      assert.deepEqual(derived.volumes, [{ kind: 'hostmount', path: '.codex' }]);
      assert.deepEqual(derived.initScripts, ['keep-default-boot']);
      return envelope('Plan', {
        profile, session: label, project: { host: cwd, guest: '/work/sample project' },
        home: { host: stage === 'wrong-home' && isClaude ? path.join(path.dirname(cwd), 'guest-codex-co') : home },
        runtime: { kind: 'hull', backend: 'hvi', available: true, bin: stage === 'wrong-runtime' ? '/another/hull' : env.BRIG_RUNTIME_BIN }, verify: { mode: 'require' },
        noPolicy: false, policies: ['accept-' + label], egress: { default: 'deny', allow: hosts.map(host => ({ host })) },
        credentials: stage === 'credentials' && isClaude ? [{ name: 'ANTHROPIC_API_KEY', state: 'forwarded' }] : [],
      });
    }
    if (args[1] === 'info') return envelope('Info', { project: { host: cwd }, workspace: guests.get(args[2]).home });
  }
  if (args[0] === 'policy') {
    if (args[1] === 'create') fs.copyFileSync(env.TUGBOAT_AGENT_POLICY, path.join(env.BRIG_POLICY_DIR, args[2] + '.yaml'));
    return answer();
  }
  if (args[0] === 'run') {
    const reference = args.at(-4);
    assert.equal(args.at(-3), cwd);
    assert.deepEqual(args.slice(-2), ['--', '--version']);
    const [profile, label] = reference.split('@');
    const wrapper = fs.readFileSync(env.BRIG_RUNTIME_BIN, 'utf8');
    assert.match(wrapper, /--store-dir/);
    assert.ok(wrapper.includes(path.join(path.dirname(cwd), 'h')));
    fs.appendFileSync(path.join(path.dirname(cwd), 'runtime-stores'), (stage === 'wrong-store' ? '/another/store' : path.join(path.dirname(cwd), 'h')) + '\n');
    const home = args[args.indexOf('--home') + 1] + '-' + label;
    fs.mkdirSync(home, { recursive: true });
    guests.set(reference, { home, running: true });
    if (stage === 'claude-boot' && profile === 'tugboat-claude') return answer('', 2, 'Claude boot unavailable');
    return answer(profile === 'tugboat-test-codex' ? 'codex-cli fixture' : 'Claude Code fixture');
  }
  if (args[0] === 'sh') {
    const guest = guests.get(args[1]);
    if (!guest?.running) return answer('', 2, 'The selected guest is stopped.');
    const kind = args[1].startsWith('tugboat-test-codex@') ? 'codex' : 'claude-code';
    const markerGuest = kind === 'codex' ? '/root/.codex/acceptance-marker' : '/brig/claude/projects/acceptance-marker';
    const markerHost = path.join(guest.home, kind === 'codex' ? '.codex/acceptance-marker' : '.claude/projects/acceptance-marker');
    if (args[2] === 'mkdir') {
      assert.deepEqual(args.slice(2), ['mkdir', '-p', path.posix.dirname(markerGuest)]);
      fs.mkdirSync(path.dirname(markerHost), { recursive: true });
      return answer();
    }
    if (args[2] === 'sh') {
      assert.deepEqual(args.slice(2), ['sh', '-c', `printf '${kind}' > '${markerGuest}'`]);
      fs.writeFileSync(markerHost, kind);
      return answer();
    }
    assert.deepEqual(args.slice(2), ['cat', markerGuest]);
    return answer(fs.readFileSync(markerHost, 'utf8'));
  }
  if (args[0] === 'stop') {
    const guest = guests.get(args[1]);
    if (stage === 'stop' && args[1].startsWith('tugboat-claude@')) return answer('', 2, 'stop unavailable');
    if (guest) guest.running = false;
    return answer();
  }
  if (args[0] === 'rm') {
    if (args[2] === '--dry-run') return answer(stage === 'preview' ? 'another-environment' : args[1]);
    if (stage === 'remove' && args[1].startsWith('tugboat-test-codex@')) return answer('', 2, 'remove unavailable');
    if (stage !== 'remaining') guests.delete(args[1]);
    return answer();
  }
  throw new Error('Unexpected acceptance command: ' + args.join(' '));
}
require('node:child_process').spawn = (binary, args, options) => {
  assert.equal(options.detached, true);
  const child = new EventEmitter();
  child.stdout = new PassThrough(); child.stderr = new PassThrough();
  queueMicrotask(() => {
    try {
      const result = fixtureCommand(binary, args, options);
      child.stdout.end(result.stdout); child.stderr.end(result.stderr);
      child.emit('close', result.status, result.signal);
    } catch (error) {
      child.emit('error', Object.assign(error, { code: 'FIXTURE_ERROR' }));
      child.stdout.end(); child.stderr.end(error.message);
      child.emit('close', 1, null);
    }
  });
  return child;
};
syncBuiltinESMExports();
