import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('../../scripts/check-rust-format.py', import.meta.url));
const formatAvailable = spawnSync('rustfmt', ['--version'], { encoding: 'utf8', timeout: 10_000 }).status === 0;
const formatOnly = { skip: formatAvailable ? false : 'requires rustfmt; the separate formatting job installs the pinned component' };

function fixture(context, baseline, updated, change = 'edit') {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-rust-format-'));
  context.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const environment = { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_SYSTEM: '/dev/null' };
  const git = (...argumentsList) => {
    const result = spawnSync('git', ['-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', ...argumentsList], {
      cwd: directory, env: environment, encoding: 'utf8', timeout: 10_000,
    });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim();
  };
  fs.mkdirSync(path.join(directory, 'src-tauri/src'), { recursive: true });
  fs.writeFileSync(path.join(directory, 'src-tauri/Cargo.toml'), 'edition = "2021"\n');
  fs.writeFileSync(path.join(directory, 'src-tauri/src/main.rs'), baseline);
  git('init', '-q');
  git('add', '.');
  git('commit', '-qm', 'baseline');
  const base = git('rev-parse', 'HEAD');
  const source = path.join(directory, 'src-tauri/src');
  const destination = change === 'move' ? 'desktop/menu.rs' : change === 'add' ? 'added.rs' : 'main.rs';
  if (change === 'move') {
    fs.mkdirSync(path.join(source, 'desktop'));
    fs.renameSync(path.join(source, 'main.rs'), path.join(source, destination));
  }
  fs.writeFileSync(path.join(source, destination), updated);
  git('add', '.');
  git('commit', '-qm', 'change');
  return spawnSync('python3', [script, base], { cwd: directory, env: environment, encoding: 'utf8', timeout: 30_000 });
}

test('the format check accepts clean additions beside existing format differences', formatOnly, context => {
  const old = 'fn existing( ){println!("old");}\n';
  const result = fixture(context, old, old + '\nfn added() {}\n');
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /checked 1 changed files/);
});

test('a deliberately malformed added Rust line fails the format check', formatOnly, context => {
  const result = fixture(context, 'fn existing() {}\n', 'fn existing() {}\n\nfn added( ){ let value=1; }\n');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /main\.rs:3: added line needs rustfmt/);
});

test('a new Rust file must have clean formatting', formatOnly, context => {
  const result = fixture(context, 'fn existing() {}\n', 'fn added( ){ let value=1; }\n', 'add');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /added\.rs:1: added line needs rustfmt/);
});

test('moving a Rust file checks edited lines and preserves existing format differences', formatOnly, context => {
  const old = 'fn existing( ){println!("old");}\nfn first() {}\nfn second() {}\nfn third() {}\n';
  const result = fixture(context, old, old + 'fn added( ){}\n', 'move');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /desktop\/menu\.rs:5: added line needs rustfmt/);
  assert.doesNotMatch(result.stderr, /menu\.rs:1:/);
});

test('a Rust parse error fails the format check', formatOnly, context => {
  const result = fixture(context, 'fn existing() {}\n', 'fn existing() {}\nfn broken(\n');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /error/);
});
