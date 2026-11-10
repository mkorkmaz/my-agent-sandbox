import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const [metadataFile, ghosttySource, ghosttyDependencies, zigDirectory] = process.argv.slice(2);
if (![metadataFile, ghosttySource, ghosttyDependencies, zigDirectory].every(Boolean)) throw new Error('Pass Cargo metadata, Ghostty source, dependencies and Zig directories.');
const root = fileURLToPath(new URL('../', import.meta.url));
const destination = path.join(root, 'resources/licenses');
fs.mkdirSync(destination, { recursive: true });
const components = [];
function collect(name, version, directory, license, repository, extraFile) {
  const folder = path.join(destination, name.replaceAll('/', '-') + '-' + version);
  fs.mkdirSync(folder, { recursive: true });
  const files = [];
  function copyNotices(source, relative = '') {
    for (const entry of fs.readdirSync(source, { withFileTypes: true })) {
      const local = path.join(relative, entry.name);
      if (entry.isDirectory() && /^(licenses?|notices?)$/i.test(entry.name)) { copyNotices(path.join(source, entry.name), local); continue; }
      if (!entry.isFile() || !/^(license|licence|copying|notice|copyright)/i.test(entry.name)) continue;
      const target = path.join(folder, local);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.copyFileSync(path.join(source, entry.name), target); files.push(local);
    }
  }
  copyNotices(directory);
  if (extraFile && fs.existsSync(extraFile) && !files.includes(path.basename(extraFile))) {
    fs.copyFileSync(extraFile, path.join(folder, path.basename(extraFile))); files.push(path.basename(extraFile));
  }
  components.push({ name, version, license, repository, notices: files.map(file => path.relative(destination, path.join(folder, file))) });
}
const metadata = JSON.parse(fs.readFileSync(metadataFile));
for (const item of metadata.packages.filter(item => item.source)) {
  collect(item.name, item.version, path.dirname(item.manifest_path), item.license, item.repository, item.license_file);
  const component = components.at(-1);
  component.sourceArchive = 'https://static.crates.io/crates/' + item.name + '/' + item.name + '-' + item.version + '.crate';
  const vcsFile = path.join(path.dirname(item.manifest_path), '.cargo_vcs_info.json');
  if (fs.existsSync(vcsFile)) component.sourceRevision = JSON.parse(fs.readFileSync(vcsFile)).git?.sha1;
  if (!component.notices.length) {
    const group = item.repository === 'https://github.com/madsmtm/objc2' ? 'objc2' : item.name;
    const supplement = path.join(root, 'resources/license-supplements', group);
    const folder = path.join(destination, item.name + '-' + item.version);
    if (fs.existsSync(supplement)) {
      for (const name of fs.readdirSync(supplement)) {
        fs.copyFileSync(path.join(supplement, name), path.join(folder, name));
        component.notices.push(path.relative(destination, path.join(folder, name)));
      }
    }
    fs.writeFileSync(path.join(folder, 'ATTRIBUTION.txt'), [item.name + ' ' + item.version, 'Authors: ' + item.authors.join(', '), 'License declaration: ' + item.license, item.repository, 'Exact source: ' + component.sourceArchive, 'Source revision: ' + component.sourceRevision, 'Upstream source is unmodified.'].join('\n') + '\n');
    component.notices.push(path.relative(destination, path.join(folder, 'ATTRIBUTION.txt')));
  }
}
const api = JSON.parse(fs.readFileSync(root + 'node_modules/@tauri-apps/api/package.json'));
collect('@tauri-apps/api', api.version, root + 'node_modules/@tauri-apps/api', api.license, api.repository.url);
const interfaceManifest = path.join(destination, 'ui-components.json');
if (fs.existsSync(interfaceManifest)) components.push(...JSON.parse(fs.readFileSync(interfaceManifest)).components);
collect('libghostty-vt', '1.0.0', ghosttySource, 'MIT', 'https://github.com/ghostty-org/ghostty');
for (const entry of fs.readdirSync(ghosttyDependencies, { withFileTypes: true }).filter(entry => entry.isDirectory())) {
  collect('ghostty-build-input-' + entry.name, 'pinned', path.join(ghosttyDependencies, entry.name), 'See notices', 'See pinned Ghostty build.zig.zon');
}
collect('zig-runtime', '0.17.1', zigDirectory, 'MIT and runtime file notices', 'https://github.com/ziglang/zig');
const hullManifest = path.join(destination, 'hull-components.json');
if (fs.existsSync(hullManifest)) components.push(...JSON.parse(fs.readFileSync(hullManifest)).components);
const text = [
  'Tugboat — Third-party notices',
  '',
  'This package includes Tauri, Rust dependencies, @tauri-apps/api, libghostty-vt, Zig runtime code, Inter, JetBrains Mono and Lucide icons.',
  'The upstream license texts are included in the named folders. Ghostty build-input notices are included conservatively.',
  'Build and test tools such as Vite, TypeScript and jsdom are not shipped in the application.',
  fs.existsSync(hullManifest) ? 'Brig and Codex are external executables. This package includes a modified hull CLI; the hvi/Vz runners remain external.' : 'Brig, hull and Codex are external executables; they are not redistributed in this package.',
  '',
  ...components.flatMap(component => [component.name + ' ' + component.version + ' — ' + (component.license ?? 'See upstream repository'), component.repository ?? '', ...component.notices, '']),
].join('\n');
fs.writeFileSync(destination + '/THIRD-PARTY-NOTICES.txt', text + '\n');
fs.writeFileSync(destination + '/components.json', JSON.stringify({ schemaVersion: 1, components }, null, 2) + '\n');
console.log(JSON.stringify({ components: components.length, missingNoticeFiles: components.filter(item => !item.notices.length).map(item => item.name) }));
