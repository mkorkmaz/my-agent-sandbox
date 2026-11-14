import fs from 'node:fs';
import path from 'node:path';
import { execFileSync, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { hashFile } from '../runtime/runtime-bundle.mjs';
import { readJson, readNotice, readNotices, root } from './license-notices.mjs';

const execute = promisify(execFile);
const goRoot = path.join(root, '.build/hull-compat');
const go = path.join(goRoot, 'go/bin/go');

function binaryModules(binary) {
  const info = JSON.parse(execFileSync(go, ['version', '-m', '-json', binary], { encoding: 'utf8', timeout: 20_000 }));
  const modules = info.Main.Path === 'github.com/brig-sh/brig' ? info.Deps : [info.Main, ...info.Deps];
  return modules.map(module => module.Replace ?? module);
}

async function collectModule({ Path: name, Version: version, Sum: checksum }) {
  const identifier = name + '@' + version;
  const { stdout } = await execute(go, ['mod', 'download', '-json', identifier], {
    cwd: goRoot, env: { ...process.env, GOTOOLCHAIN: 'local', GOPATH: path.join(goRoot, 'go-path') },
    encoding: 'utf8', timeout: 180_000,
  });
  const module = JSON.parse(stdout);
  if (module.Path !== name || module.Version !== version || !module.Dir || (checksum && module.Sum !== checksum)) {
    throw new Error('Go source mismatch: ' + identifier);
  }
  const source = 'https://proxy.golang.org/' + name.replace(/[A-Z]/g, letter => '!' + letter.toLowerCase()) + '/@v/' + version + '.zip';
  const notices = readNotices(module.Dir);
  if (!notices.length) {
    notices.push({ name: 'ATTRIBUTION.txt', text: identifier + '\nExact source: ' + source + '\nSource checksum: ' + module.Sum + '\nThe upstream module archive contains no root license notice. See the exact source for file-level terms.\n' });
  }
  return { name, version, source, notices };
}

export async function collectRuntimeLicenses(bin) {
  const pins = readJson(path.join(root, 'config/brig-runtime.json'));
  const brig = readJson(path.join(root, 'config/brig-compat.json'));
  const modules = new Map();
  for (const name of ['brig', 'brigd', 'brig-broker', 'cosign']) {
    for (const module of binaryModules(path.join(bin, name))) {
      modules.set(module.Path + '@' + module.Version, module);
    }
  }
  const queue = [...modules.values()];
  const components = [];
  while (queue.length) components.push(...await Promise.all(queue.splice(0, 6).map(collectModule)));

  const source = path.join(root, '.build/brig-compat/brig-' + brig.commit);
  const modifications = 'Tugboat modifies Brig ' + brig.version + ' to find the bundled mke2fs beside its executable and preserve the ' + brig.version + ' API version string while reporting modified:true. The CLI, daemon and broker use the same patched source. The patch and source pin are in sources/brig/ beside this notice file.\n';
  components.push({
    name: 'github.com/brig-sh/brig', version: brig.version, source: brig.source.url, revision: brig.commit,
    notices: [...readNotices(source), { name: 'MODIFICATIONS.txt', text: modifications }],
  });
  const runnerLicense = path.join(root, '.build/brig-runtime-downloads/hvi-LICENSE');
  if (!fs.existsSync(runnerLicense) || hashFile(runnerLicense) !== pins.hull.hviLicenseSha256) {
    execFileSync('/usr/bin/curl', ['-fsSL', '--max-time', '60', 'https://raw.githubusercontent.com/brig-sh/hvi-vmm/v0.1.0-rc.1/LICENSE', '-o', runnerLicense], { timeout: 65_000 });
  }
  if (hashFile(runnerLicense) !== pins.hull.hviLicenseSha256) throw new Error('hvi license SHA-256 mismatch.');
  const runnerNotices = readNotices(path.join(root, '.build/brig-runtime-inputs/hull'));
  components.push({ name: 'hull-runners', version: pins.hull.version, source: pins.hull.url, notices: [...runnerNotices, readNotice(runnerLicense, 'HVI-LICENSE')] });
  return components;
}

export function packageCorrespondingSource(destination) {
  const disk = readJson(path.join(root, 'config/disk-tools.json'));
  const work = path.join(root, '.build/disk-tools');
  const archive = path.join(work, 'downloads/e2fsprogs-' + disk.version + '.tar.gz');
  if (hashFile(archive) !== disk.source.sha256) throw new Error('e2fsprogs source archive checksum mismatch.');
  const diskFiles = path.join(destination, 'sources/e2fsprogs');
  fs.mkdirSync(diskFiles, { recursive: true });
  fs.copyFileSync(archive, path.join(diskFiles, 'source.tar.gz'));
  fs.copyFileSync(path.join(root, 'scripts/runtime/prepare-disk-tools.mjs'), path.join(diskFiles, 'prepare-disk-tools.mjs'));
  fs.copyFileSync(path.join(root, 'config/disk-tools.json'), path.join(diskFiles, 'disk-tools.json'));
  const instructions = [
    'Build the bundled e2fsprogs ' + disk.version + ' disk tools',
    '',
    'Use an Apple silicon Mac with Node.js and the Xcode Command Line Tools.',
    'Copy this folder to a writable location. From that copy, run:',
    '',
    '  mkdir -p scripts/runtime config .build/disk-tools/downloads',
    '  cp prepare-disk-tools.mjs scripts/runtime/',
    '  cp disk-tools.json config/',
    '  cp source.tar.gz .build/disk-tools/downloads/e2fsprogs-' + disk.version + '.tar.gz',
    '  node scripts/runtime/prepare-disk-tools.mjs',
    '',
    'The script verifies the source archive and builds mke2fs, debugfs and e2fsck in .build/disk-tools/bin/.',
  ];
  fs.writeFileSync(path.join(diskFiles, 'BUILD.txt'), instructions.join('\n') + '\n');
  const brigFiles = path.join(destination, 'sources/brig');
  fs.mkdirSync(brigFiles, { recursive: true });
  fs.copyFileSync(path.join(root, 'resources/runtime/brig-disk-tools.patch'), path.join(brigFiles, 'brig-disk-tools.patch'));
  fs.copyFileSync(path.join(root, 'config/brig-compat.json'), path.join(brigFiles, 'brig-compat.json'));
  return {
    name: 'e2fsprogs', version: disk.version, source: disk.source.url,
    notices: readNotices(path.join(work, 'e2fsprogs-' + disk.version)),
  };
}
