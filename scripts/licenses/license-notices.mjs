import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

export const root = fileURLToPath(new URL('../../', import.meta.url));
export const readJson = file => JSON.parse(fs.readFileSync(file, 'utf8'));

// Keep the original text, including a possible UTF-8 BOM and all line endings.
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
export function readNotice(file, name = path.basename(file)) {
  return { name, text: utf8.decode(fs.readFileSync(file)) };
}

export function readNotices(directory, relative = '') {
  return fs.readdirSync(path.join(directory, relative), { withFileTypes: true }).flatMap(entry => {
    const name = path.join(relative, entry.name);
    if (entry.isDirectory() && /^(licen[cs]es?|notices?)$/i.test(entry.name)) return readNotices(directory, name);
    if (!entry.isFile() || (!relative && !/^(licen[cs]e|copying|notice|copyright|patents)/i.test(entry.name))) return [];
    return [readNotice(path.join(directory, name), name)];
  });
}

export function renderNotices(components) {
  const texts = new Map();
  const index = [];
  for (const component of [...components].sort((a, b) => label(a).localeCompare(label(b), 'en'))) {
    if (!component.notices.length) throw new Error('Missing notices for ' + label(component));
    index.push('', label(component));
    if (component.license) index.push('License: ' + component.license);
    if (component.source) index.push('Source: ' + component.source);
    if (component.revision) index.push('Revision: ' + component.revision);
    if (component.modifications) index.push('Modifications: ' + component.modifications);
    for (const { name, text } of [...component.notices].sort((a, b) => a.name.localeCompare(b.name, 'en'))) {
      if (!text.trim()) throw new Error('Empty notice for ' + label(component) + ': ' + name);
      if (!texts.has(text)) texts.set(text, texts.size + 1);
      index.push('  [' + texts.get(text) + '] ' + name);
    }
  }
  const sections = ['Tugboat — Third-party notices', 'Component index', ...index, '', 'Notice texts'];
  for (const [text, number] of texts) sections.push('', '===== Notice ' + number + ' =====', text);
  return sections.join('\n') + '\n';
}

function label(component) {
  return component.name + ' ' + component.version;
}

export function verifyPackagedLicenses(app, diskTools) {
  const directory = path.join(app, 'Contents/Resources/licenses');
  const expected = [
    'THIRD-PARTY-NOTICES.txt', 'sources/e2fsprogs/BUILD.txt', 'sources/e2fsprogs/source.tar.gz',
    'sources/e2fsprogs/prepare-disk-tools.mjs', 'sources/e2fsprogs/disk-tools.json',
    'sources/brig/brig-disk-tools.patch', 'sources/brig/brig-compat.json',
  ];
  const files = fs.readdirSync(directory, { recursive: true }).filter(file => fs.statSync(path.join(directory, file)).isFile());
  if (files.length !== expected.length || !expected.every(file => files.includes(file))) throw new Error('Unexpected contents of packaged licenses folder.');
  const disk = path.join(directory, 'sources/e2fsprogs');
  const archiveHash = createHash('sha256').update(fs.readFileSync(path.join(disk, 'source.tar.gz'))).digest('hex');
  if (archiveHash !== diskTools.source.sha256) throw new Error('Packaged e2fsprogs source checksum mismatch.');
  const notices = fs.readFileSync(path.join(directory, 'THIRD-PARTY-NOTICES.txt'), 'utf8');
  if (!notices.includes('\n===== Notice 1 =====\n') || !notices.includes('\ne2fsprogs ' + diskTools.version + '\n')) {
    throw new Error('Packaged third-party notice texts are missing.');
  }
  const instructions = fs.readFileSync(path.join(disk, 'BUILD.txt'), 'utf8');
  if (!/\bnode scripts\/(?:runtime\/)?prepare-disk-tools\.mjs\b/.test(instructions)) {
    throw new Error('Packaged e2fsprogs build instructions are missing.');
  }
}
