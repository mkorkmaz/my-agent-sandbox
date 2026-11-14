import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { readJson, readNotices, root } from './license-notices.mjs';

export function collectHullLicenses() {
  const pin = readJson(path.join(root, 'config/hull-compat.json'));
  const work = path.join(root, '.build/hull-compat');
  const source = path.join(work, 'hull-' + pin.commit);
  const goRoot = path.join(work, 'go');
  const format = '{{if and (not .Standard) .Module}}{{.Module.Path}}\t{{.Module.Version}}\t{{.Module.Dir}}{{end}}';
  const output = execFileSync(path.join(goRoot, 'bin/go'), ['list', '-mod=readonly', '-deps', '-f', format, './cmd/hull'], {
    cwd: source, encoding: 'utf8', timeout: 180_000,
    env: { ...process.env, GOTOOLCHAIN: 'local', GOCACHE: path.join(work, 'go-cache'), GOPATH: path.join(work, 'go-path'), CGO_ENABLED: '0', GOOS: 'darwin', GOARCH: 'arm64' },
  });
  const modules = new Set(output.trim().split('\n').filter(Boolean));
  const components = [];
  for (const row of modules) {
    const [name, moduleVersion, directory] = row.split('\t');
    const version = moduleVersion || pin.version + '-desktop-spaces-1';
    const local = directory === source || directory.startsWith(source + path.sep);
    const notices = readNotices(local ? source : directory);
    if (!notices.length) {
      notices.push({ name: 'ATTRIBUTION.txt', text: name + ' ' + version + '\nExact module source: https://proxy.golang.org/' + name + '/@v/' + moduleVersion + '.zip\nThe upstream module archive contains no license notice. No license is inferred.\n' });
    }
    if (name === 'github.com/brig-sh/hull') {
      notices.push({ name: 'MODIFICATIONS.txt', text: 'Tugboat modifies hull ' + pin.version + ' (' + pin.commit + ') to support spaces in shared guest paths, quote those paths in legacy mount commands and locate bundled ext4 disk tools beside hull.\nPatches: https://github.com/brig-sh/hull/tree/' + pin.commit + ' plus resources/runtime/hull-spaces.patch and resources/runtime/hull-disk-tools.patch in the Tugboat source.\nTugboat preserves the upstream signatures on the bundled hvi/Vz runners. Brig modifications are described in the bundled runtime source notices.\n' });
    }
    components.push({ name, version, source: local ? pin.source.url : 'https://' + name, notices });
  }
  components.push({ name: 'go-runtime', version: pin.go.version, source: pin.go.url, notices: readNotices(goRoot) });
  return components;
}
