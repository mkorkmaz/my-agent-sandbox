import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { collectHullLicenses } from './collect-hull-licenses.mjs';
import { collectRuntimeLicenses, packageCorrespondingSource } from './collect-runtime-licenses.mjs';
import { readJson, readNotice, readNotices, renderNotices, root } from './license-notices.mjs';

const supplements = path.join(root, 'resources/license-supplements');
const { ghostty, target } = readJson(path.join(root, 'config/development-versions.json'));

function readSupplement(folder) {
  if (!fs.existsSync(folder)) return [];
  return fs.readdirSync(folder, { withFileTypes: true }).filter(entry => entry.isFile() && !entry.name.startsWith('.'))
    .map(entry => readNotice(path.join(folder, entry.name)));
}

function collectCrate(item) {
  const directory = path.dirname(item.manifest_path);
  const notices = readNotices(directory);
  if (item.license_file && !notices.some(notice => notice.name === item.license_file)) {
    notices.push(readNotice(path.resolve(directory, item.license_file)));
  }
  const source = 'https://static.crates.io/crates/' + item.name + '/' + item.name + '-' + item.version + '.crate';
  const vcsFile = path.join(directory, '.cargo_vcs_info.json');
  const revision = fs.existsSync(vcsFile) ? readJson(vcsFile).git?.sha1 : undefined;
  if (!notices.length) {
    const group = item.repository === 'https://github.com/madsmtm/objc2' ? 'objc2' : item.name;
    notices.push(...readSupplement(path.join(supplements, group)));
    const attribution = [item.name + ' ' + item.version, 'Authors: ' + item.authors.join(', '), 'License declaration: ' + item.license, item.repository, 'Exact source: ' + source];
    if (revision) attribution.push('Source revision: ' + revision);
    notices.push({ name: 'ATTRIBUTION.txt', text: [...attribution, 'Upstream source is unmodified.'].join('\n') + '\n' });
  }
  notices.push(...readSupplement(path.join(supplements, item.name + '-' + item.version)));
  return { name: item.name, version: item.version, license: item.license, source, revision, notices };
}

function collectInterface() {
  const components = Object.keys(readJson(path.join(root, 'package.json')).dependencies).map(name => {
    const directory = path.join(root, 'node_modules', name);
    const item = readJson(path.join(directory, 'package.json'));
    return { name, version: item.version, license: item.license, source: item.repository?.url ?? item.repository, notices: readNotices(directory) };
  });
  for (const item of readJson(path.join(supplements, 'ui-components.json')).components) {
    const notices = item.notices.map(file => readNotice(path.join(supplements, file)));
    components.push({ ...item, source: item.sourceArchive, revision: item.sourceRevision, notices });
  }
  return components;
}

function collectGhostty() {
  const work = path.join(root, '.build/libghostty');
  const source = path.join(work, 'libghostty-vt-' + ghostty.version);
  const zig = path.join(work, 'zig-aarch64-macos-' + ghostty.zig.version);
  const components = [
    { name: 'libghostty-vt', version: ghostty.version, license: 'MIT', source: ghostty.source.url, notices: readNotices(source) },
    { name: 'zig-runtime', version: ghostty.zig.version, source: ghostty.zig.tarball, notices: readNotices(zig) },
  ];
  const inputs = source + '-deps';
  for (const entry of fs.readdirSync(inputs, { withFileTypes: true }).filter(entry => entry.isDirectory())) {
    components.push({ name: 'ghostty-build-input-' + entry.name, version: 'pinned', source: ghostty.dependencies.url, notices: readNotices(path.join(inputs, entry.name)) });
  }
  return components;
}

const metadata = execFileSync('cargo', ['metadata', '--locked', '--format-version', '1', '--filter-platform', target.rust,
  '--features', 'tauri/custom-protocol', '--manifest-path', path.join(root, 'src-tauri/Cargo.toml')], {
  encoding: 'utf8', timeout: 180_000, maxBuffer: 32 * 1024 * 1024,
});
const work = path.join(root, '.build');
const destination = path.join(work, 'licenses');
const staging = fs.mkdtempSync(path.join(work, 'package-licenses-'));
const next = path.join(staging, 'licenses');
try {
  const components = [
    ...JSON.parse(metadata).packages.filter(item => item.source).map(collectCrate),
    ...collectInterface(),
    ...collectGhostty(),
    ...collectHullLicenses(),
    ...await collectRuntimeLicenses(path.join(work, 'brig-runtime/bin')),
    packageCorrespondingSource(next),
  ];
  fs.writeFileSync(path.join(next, 'THIRD-PARTY-NOTICES.txt'), renderNotices(components));
  const previous = path.join(staging, 'previous');
  if (fs.existsSync(destination)) fs.renameSync(destination, previous);
  try { fs.renameSync(next, destination); }
  catch (error) {
    if (fs.existsSync(previous)) fs.renameSync(previous, destination);
    throw error;
  }
  console.log('Generated ' + path.join(destination, 'THIRD-PARTY-NOTICES.txt'));
} finally {
  fs.rmSync(staging, { recursive: true, force: true });
}
