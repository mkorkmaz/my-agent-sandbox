import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const directory = process.env.TUGBOAT_DISK_TOOLS_DIRECTORY ?? path.join(root, '.build/disk-tools/bin');
const configuration = process.env.TUGBOAT_DISK_TOOLS_CONFIG ?? path.join(root, '.build/disk-tools/etc/mke2fs.conf');
const available = process.platform === 'darwin' && fs.existsSync(path.join(directory, 'mke2fs'));

test('portable disk tools create, edit and repair an ext4 image without Homebrew', { skip: available ? false : 'requires macOS and prepared disk tools' }, context => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat disk tools '));
  context.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const environment = { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', LANG: 'en_US.UTF-8', MKE2FS_CONFIG: configuration };
  const run = (name, argumentsList, expected = [0]) => {
    const result = spawnSync(path.join(directory, name), argumentsList, { env: environment, encoding: 'utf8', timeout: 30_000 });
    assert.equal(result.error, undefined, name + ': ' + result.error);
    assert.ok(expected.includes(result.status), name + ': ' + result.stdout + result.stderr);
    return result.stdout + result.stderr;
  };
  const disk = path.join(temporary, 'fresh disk.ext4');
  const payload = path.join(temporary, 'payload file');
  const contents = 'Tugboat portable ext4 check\n';
  fs.writeFileSync(payload, contents);
  run('mke2fs', ['-q', '-F', '-t', 'ext4', '-m', '0', disk, '32M']);
  const edited = run('debugfs', ['-w', '-R', 'write "' + payload + '" /probe', disk]);
  assert.match(edited, /Allocated inode/);
  assert.ok(run('debugfs', ['-R', 'cat /probe', disk]).includes(contents));
  run('e2fsck', ['-f', '-y', disk], [0, 1, 2]);
  run('e2fsck', ['-f', '-n', disk]);
  for (const name of ['mke2fs', 'debugfs', 'e2fsck']) {
    const dependencies = spawnSync('/usr/bin/otool', ['-L', path.join(directory, name)], { encoding: 'utf8', timeout: 10_000 });
    assert.equal(dependencies.status, 0);
    assert.doesNotMatch(dependencies.stdout, /\/opt\/homebrew\/|\/usr\/local\//);
    const build = spawnSync('/usr/bin/vtool', ['-show-build', path.join(directory, name)], { encoding: 'utf8', timeout: 10_000 });
    assert.equal(build.status, 0);
    assert.match(build.stdout, /minos 15\.0\b/);
  }
});
