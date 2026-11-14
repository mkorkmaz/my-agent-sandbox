import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const scripts = fileURLToPath(new URL('../../scripts/acceptance/', import.meta.url));
const requiredTools = ['tugboat', 'brig', 'brigd', 'brig-broker', 'hull', 'hvi', 'vz-runner', 'cosign', 'mke2fs', 'debugfs', 'e2fsck'];
const macOnly = { skip: process.platform !== 'darwin' || process.arch !== 'arm64' };

function fixture(t, stage) {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'tgb-acceptance-fixture-'));
  const root = fs.mkdtempSync('/private/tmp/tgb-');
  t.after(() => { fs.rmSync(workspace, { recursive: true, force: true }); fs.rmSync(root, { recursive: true, force: true }); });
  const application = path.join(workspace, 'App with spaces/Tugboat.app');
  const bin = path.join(application, 'Contents/MacOS');
  const resources = path.join(application, 'Contents/Resources/runtime');
  fs.mkdirSync(bin, { recursive: true }); fs.mkdirSync(resources, { recursive: true });
  for (const name of requiredTools) fs.writeFileSync(path.join(bin, name), '#!/bin/sh\n', { mode: 0o700 });
  fs.writeFileSync(path.join(resources, 'build.json'), '{}\n');
  fs.mkdirSync(path.join(workspace, 'scripts/acceptance'), { recursive: true });
  for (const name of ['check-bundled-runtime.mjs', 'acceptance-process.mjs']) fs.copyFileSync(path.join(scripts, name), path.join(workspace, 'scripts/acceptance', name));
  const stub = path.join(workspace, 'commands.cjs');
  fs.writeFileSync(stub, `const {EventEmitter}=require('node:events');
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict');
const originalExists=fs.existsSync;
if(process.env.TGB_FIXTURE_STAGE.startsWith('clean-mac-')) fs.existsSync=file=>{
  if(['/opt/homebrew/bin/brew','/opt/homebrew/Homebrew/bin/brew','/usr/local/bin/brew','/usr/local/Homebrew/bin/brew'].includes(file)) return process.env.TGB_FIXTURE_STAGE==='clean-mac-reject';
  return originalExists(file);
};
require('node:child_process').spawn=(binary,args,options)=>{
  const sandboxed=binary==='/usr/bin/sandbox-exec';
  let program=binary,programArguments=args;
  if(sandboxed){
    assert.equal(args[0],'-f');
    const profile=fs.readFileSync(args[1],'utf8');
    assert.match(profile,/\\(subpath "\\/opt\\/homebrew"\\)/);
    assert.match(profile,/\\(subpath "\\/usr\\/local"\\)/);
    assert.match(profile,/\\(deny network\\*\\)/);
    program=args[2];programArguments=args.slice(3);
    assert.ok(['/bin/cat','/bin/sh'].includes(program)||path.basename(program)==='tugboat');
  }
  assert.ok(options.env.HOME.startsWith('/private/tmp/tgb-'));
  assert.ok(!options.env.PATH.includes('/opt/homebrew')&&!options.env.PATH.includes('/usr/local'));
  assert.equal(options.env.OPENAI_API_KEY,undefined);
  assert.equal(options.env.ANTHROPIC_API_KEY,undefined);
  const child=new EventEmitter();child.stdout=new EventEmitter();child.stderr=new EventEmitter();
  child.stdout.setEncoding=child.stderr.setEncoding=()=>{};
  process.nextTick(()=>{
    if(program==='/bin/cat'||program==='/bin/sh') { assert.equal(sandboxed,true);child.stderr.emit('data','Operation not permitted');child.emit('close',1,null);return; }
    if(path.basename(program)==='brig') {
      assert.equal(sandboxed,false,'Guest commands must let hvi apply its own sandbox');
      const command=programArguments;
      const reply=value=>{child.stdout.emit('data',JSON.stringify(value));child.emit('close',0,null);};
      if(command[0]==='--json'&&command[1]==='ls') return reply({apiVersion:'brig.sh/v1alpha1',kind:'SandboxList',data:{sandboxes:[]}});
      if(command[0]==='agent') return reply({name:'codex',binary:'codex',env:[]});
      if(command[0]==='--json'&&command[1]==='plan') return reply({apiVersion:'brig.sh/v1alpha1',kind:'Plan',data:{project:{host:options.cwd,guest:'/work/'+path.basename(options.cwd)},runtime:{bin:options.env.BRIG_RUNTIME_BIN,backend:'hvi',available:true},isolation:'microVM (hull, hvi backend, rootfs virtiofs)'}});
      assert.equal(command[0],'policy','Acceptance must stop before guest boot');
      child.emit('close',0,null);return;
    }
    assert.equal(path.basename(program),'tugboat'); assert.equal(sandboxed,true);assert.deepEqual(programArguments,['--runtime-check']);
    if(process.env.TGB_FIXTURE_STAGE==='runtime-error'){child.stderr.emit('data','Damaged runtime');child.emit('close',1,null);return;}
    const bin=path.dirname(program);
    child.stdout.emit('data',JSON.stringify({brigSource:'bundled',brigStatus:'installed',tools:{brig:process.env.TGB_FIXTURE_STAGE==='wrong-path'?'/opt/homebrew/bin/brig':path.join(bin,'brig'),hull:path.join(bin,'hull')},version:{data:{version:process.env.TGB_FIXTURE_STAGE==='wrong-version'?'v9.0.0':'v0.5.2'}}}));
    child.emit('close',0,null);
  });
  return child;
};
`);
  if (stage === 'missing-tool') fs.unlinkSync(path.join(bin, 'hvi'));
  let entry = path.join(workspace, 'scripts/acceptance/check-bundled-runtime.mjs');
  if (stage.startsWith('cache-')) {
    const sourceRoot = fs.mkdtempSync('/private/tmp/tgb-');
    t.after(() => fs.rmSync(sourceRoot, { recursive: true, force: true }));
    const cachedStore = path.join(sourceRoot, 'h.sparseimage');
    fs.writeFileSync(cachedStore, 'synthetic cache');
    const original = {
      root: sourceRoot, completedAt: new Date().toISOString(),
      manifestSha256: stage === 'cache-wrong-runtime' ? 'different' : createHash('sha256').update('{}\n').digest('hex'),
      remaining: stage === 'cache-live-guest' ? [{ ref: 'live' }] : [],
      commands: [{ binary: '/fixture/hull', args: ['--store-dir', sourceRoot + '/h', 'store', 'detach'], exitCode: stage === 'cache-attached' ? 1 : 0, error: null, signal: null }],
    };
    const originalEvidence = path.join(workspace, 'artifacts/bundled-runtime');
    fs.mkdirSync(originalEvidence, { recursive: true });
    fs.writeFileSync(path.join(originalEvidence, path.basename(sourceRoot) + '.json'), JSON.stringify(original));
    entry = path.join(workspace, 'scripts/acceptance/check-cache.mjs');
    fs.writeFileSync(entry, 'import { checkBundledRuntime } from "./check-bundled-runtime.mjs";\nawait checkBundledRuntime(process.argv[2], process.argv[3], { cachedStore: ' + JSON.stringify(cachedStore) + ' });\n');
  }
  const argumentsList = ['--require', stub, entry, application, root];
  if (stage.startsWith('clean-mac-')) argumentsList.push('--clean-mac');
  const result = spawnSync(process.execPath, argumentsList, {
    encoding: 'utf8', timeout: 10_000,
    env: { ...process.env, TGB_FIXTURE_STAGE: stage, OPENAI_API_KEY: 'fixture-secret', ANTHROPIC_API_KEY: 'fixture-secret' },
  });
  assert.ifError(result.error);
  const evidence = path.join(workspace, 'artifacts/bundled-runtime', path.basename(root) + '.json');
  return { result, root, report: fs.existsSync(evidence) ? JSON.parse(fs.readFileSync(evidence)) : null };
}

test('bundled acceptance rejects missing tools before creating test state', macOnly, t => {
  const { result, root, report } = fixture(t, 'missing-tool');
  assert.notEqual(result.status, 0);
  assert.deepEqual(fs.readdirSync(root), []);
  assert.equal(report, null);
});

test('bundled acceptance stops before boot when offline inspection selects a system CLI or wrong version', macOnly, t => {
  for (const stage of ['wrong-path', 'wrong-version', 'runtime-error']) {
    const { result, report } = fixture(t, stage);
    assert.equal(result.status, 1, result.stderr);
    assert.equal(report.outcome, 'failed');
    assert.ok(report.failure);
    assert.ok(report.completedAt);
    assert.equal(report.homebrewAccessBlocked, false);
    assert.equal(report.preflightHomebrewAccessBlocked, true);
    assert.equal(report.separateCleanMacTest, false);
    assert.equal(report.minimumMacOS15Test, false);
    assert.ok(report.commands.every(command => !command.args.includes('run')));
    assert.equal(report.commands.at(-1).offline, true);
    assert.equal(report.tools.hvi.path.endsWith('/Contents/MacOS/hvi'), true);
    assert.equal(JSON.stringify(report).includes('fixture-secret'), false);
  }
});

test('bundled acceptance rejects a virtiofs fallback before the first boot', macOnly, t => {
  const { result, report } = fixture(t, 'virtiofs-plan');
  assert.equal(result.status, 1, result.stderr);
  assert.match(report.failure, /must select the ext4 overlay root/);
  assert.ok(report.commands.every(command => !command.args.includes('run')));
  assert.ok(report.commands.filter(command => path.basename(command.binary) === 'brig').every(command => command.preflightSandbox === false));
});

test('clean Mac mode rejects Homebrew before test state and records the user-provided context only after absence checks', macOnly, t => {
  const rejected = fixture(t, 'clean-mac-reject');
  assert.equal(rejected.result.status, 1);
  assert.match(rejected.result.stderr, /requires Homebrew to be absent/);
  assert.deepEqual(fs.readdirSync(rejected.root), []);
  assert.equal(rejected.report, null);
  const accepted = fixture(t, 'clean-mac-no-brew');
  assert.equal(accepted.result.status, 1);
  assert.match(accepted.report.failure, /must select the ext4 overlay root/);
  assert.equal(accepted.report.homebrewInstalledOnHost, false);
  assert.equal(accepted.report.separateCleanMacTest, true);
  assert.deepEqual(accepted.report.cleanMacContext, { source: 'user-provided', homebrewAbsentAtStandardPaths: true });
  assert.match(accepted.report.host.macOSVersion, /^\d+\.\d+/);
});

test('cache retries reject a different bundle, a live guest or a failed detach before creating the new test state', macOnly, t => {
  for (const stage of ['cache-wrong-runtime', 'cache-live-guest', 'cache-attached']) {
    const { result, root, report } = fixture(t, stage);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /same runtime bundle|no remaining guests|must be detached/);
    assert.deepEqual(fs.readdirSync(root), []);
    assert.equal(report, null);
  }
});
