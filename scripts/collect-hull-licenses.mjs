import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const [source, go] = process.argv.slice(2);
if (!source || !go) throw new Error('Pass the hull source directory and Go executable.');
const root = fileURLToPath(new URL('../', import.meta.url));
const pin = JSON.parse(fs.readFileSync(path.join(root, 'config/hull-compat.json')));
const destination = path.join(root, 'resources/licenses');
const format = '{{if and (not .Standard) .Module}}{{.Module.Path}}\t{{.Module.Version}}\t{{.Module.Dir}}{{end}}';
const modules = execFileSync(go, ['list', '-mod=readonly', '-deps', '-f', format, './cmd/hull'], { cwd: source, encoding: 'utf8' }).trim().split('\n').filter(Boolean);
const components = [];
const seen = new Set();
for (const row of [...modules, 'go-runtime\t' + pin.go.version + '\t' + path.dirname(path.dirname(go))]) {
  const [name, moduleVersion, directory] = row.split('\t');
  if (seen.has(name)) continue;
  seen.add(name);
  const version = moduleVersion || pin.version + '-desktop-spaces-1';
  const folder = path.join(destination, 'hull-module-' + name.replaceAll('/', '-') + '-' + version);
  fs.mkdirSync(folder, { recursive: true });
  const notices = [];
  const noticeDirectory = name.startsWith('github.com/brig-sh/hull/') && path.resolve(directory).startsWith(path.resolve(source) + path.sep) ? source : directory;
  for (const entry of fs.readdirSync(noticeDirectory, { withFileTypes: true })) {
    if (!entry.isFile() || !/^(license|licence|copying|notice|copyright|patents)/i.test(entry.name)) continue;
    const target = path.join(folder, entry.name);
    if (fs.existsSync(target)) fs.chmodSync(target, 0o644);
    fs.copyFileSync(path.join(noticeDirectory, entry.name), target);
    fs.chmodSync(target, 0o644);
    notices.push(path.relative(destination, target));
  }
  const missingLicense = !notices.length;
  if (missingLicense) {
    const target = path.join(folder, 'ATTRIBUTION.txt');
    fs.writeFileSync(target, name + ' ' + version + '\nExact module source: https://proxy.golang.org/' + name + '/@v/' + moduleVersion + '.zip\nThe upstream module archive contains no license notice. No license is inferred.\n');
    notices.push(path.relative(destination, target));
  }
  const repository = name === 'go-runtime' ? 'https://go.dev' : 'https://' + name;
  components.push({ name, version, repository, license: missingLicense ? 'Not declared in module archive' : 'See included notices', notices, distribution: 'bundled-hull' });
}
const hullFolder = path.join(destination, 'hull-module-github.com-brig-sh-hull-' + pin.version + '-desktop-spaces-1');
const modifications = 'Tugboat modifies hull ' + pin.version + ' (' + pin.commit + ') to support spaces in shared guest paths and quote those paths in legacy mount commands.\nPatch: https://github.com/brig-sh/hull/tree/' + pin.commit + ' plus resources/runtime/hull-spaces.patch in the Tugboat source.\nThe external Brig CLI and hvi/Vz runners are not modified or redistributed.\n';
fs.writeFileSync(path.join(hullFolder, 'MODIFICATIONS.txt'), modifications);
components.find(item => item.name === 'github.com/brig-sh/hull').notices.push(path.relative(destination, path.join(hullFolder, 'MODIFICATIONS.txt')));
fs.writeFileSync(path.join(destination, 'hull-components.json'), JSON.stringify({ schemaVersion: 1, components }, null, 2) + '\n');
const file = path.join(destination, 'components.json');
const main = JSON.parse(fs.readFileSync(file));
main.components = main.components.filter(item => item.distribution !== 'bundled-hull').concat(components);
fs.writeFileSync(file, JSON.stringify(main, null, 2) + '\n');
const noticesFile = path.join(destination, 'THIRD-PARTY-NOTICES.txt');
const text = fs.readFileSync(noticesFile, 'utf8').split('\nBundled hull CLI\n')[0].replace('Brig, hull and Codex are external executables; they are not redistributed in this package.', 'Brig and Codex are external executables. This package includes a modified hull CLI; the hvi/Vz runners remain external.');
fs.writeFileSync(noticesFile, text.trimEnd() + '\n\nBundled hull CLI\n' + modifications + '\n' + components.flatMap(item => [item.name + ' ' + item.version, ...item.notices, '']).join('\n'));
console.log('Collected licenses for ' + components.length + ' hull/Go components.');
