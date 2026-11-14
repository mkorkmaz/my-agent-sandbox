import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { readNotice, renderNotices, verifyPackagedLicenses } from '../../scripts/licenses/license-notices.mjs';

test('components share identical notices and retain distinct notices from the same license', () => {
  const shared = { name: 'LICENSE', text: 'Permission granted.\r\n' };
  const first = { name: 'Alpha', version: '1', notices: [shared, { name: 'NOTICE', text: 'Copyright Alice\n' }] };
  const second = { name: 'Beta', version: '2', modifications: 'Patched by Tugboat.', notices: [
    { ...shared, name: 'COPYING' }, { name: 'NOTICE', text: 'Copyright Bob\n' },
  ] };
  const result = renderNotices([second, first]);
  assert.equal(result, renderNotices([first, { ...second, notices: second.notices.toReversed() }]));
  assert.match(result, /Alpha 1\n  \[1\] LICENSE\n  \[2\] NOTICE/);
  assert.match(result, /Beta 2\nModifications: Patched by Tugboat\.\n  \[1\] COPYING\n  \[3\] NOTICE/);
  assert.equal(result.split(shared.text).length, 2);
  assert.ok(result.includes('===== Notice 2 =====\nCopyright Alice\n'));
  assert.ok(result.includes('===== Notice 3 =====\nCopyright Bob\n'));
});

test('collecting notices preserves a UTF-8 BOM and rejects malformed or missing texts', t => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-notices-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const file = path.join(directory, 'LICENSE');
  fs.writeFileSync(file, '\uFEFFCopyright\r\n');
  assert.equal(readNotice(file).text, '\uFEFFCopyright\r\n');
  fs.writeFileSync(file, Buffer.from([0xff]));
  assert.throws(() => readNotice(file), /utf-8/i);
  assert.throws(() => renderNotices([{ name: 'Example', version: '1', notices: [] }]), /Missing notices for Example 1/);
  assert.throws(() => renderNotices([{ name: 'Example', version: '1', notices: [{ name: 'LICENSE', text: '\n' }] }]), /Empty notice for Example 1/);
});

test('the package verifier rejects stale notices, a missing recipe, and a different source archive', t => {
  const app = fs.mkdtempSync(path.join(os.tmpdir(), 'tugboat-license-package-'));
  t.after(() => fs.rmSync(app, { recursive: true, force: true }));
  const directory = path.join(app, 'Contents/Resources/licenses');
  const disk = path.join(directory, 'sources/e2fsprogs');
  const brig = path.join(directory, 'sources/brig');
  fs.mkdirSync(disk, { recursive: true });
  fs.mkdirSync(brig);
  const source = Buffer.from('archive fixture');
  const pin = { version: '1', source: { sha256: createHash('sha256').update(source).digest('hex') } };
  const notices = renderNotices([{ name: 'e2fsprogs', version: pin.version, notices: [{ name: 'NOTICE', text: 'License terms' }] }]);
  fs.writeFileSync(path.join(directory, 'THIRD-PARTY-NOTICES.txt'), notices);
  fs.writeFileSync(path.join(disk, 'source.tar.gz'), source);
  fs.writeFileSync(path.join(disk, 'BUILD.txt'), 'Run node scripts/runtime/prepare-disk-tools.mjs');
  fs.writeFileSync(path.join(disk, 'disk-tools.json'), JSON.stringify(pin));
  fs.writeFileSync(path.join(disk, 'prepare-disk-tools.mjs'), '// build fixture');
  fs.writeFileSync(path.join(brig, 'brig-compat.json'), '{}');
  fs.writeFileSync(path.join(brig, 'brig-disk-tools.patch'), 'patch fixture');
  verifyPackagedLicenses(app, pin);
  fs.writeFileSync(path.join(disk, 'BUILD.txt'), 'Run node scripts/prepare-disk-tools.mjs');
  verifyPackagedLicenses(app, pin);
  fs.writeFileSync(path.join(disk, 'BUILD.txt'), 'No build command');
  assert.throws(() => verifyPackagedLicenses(app, pin), /build instructions/);
  fs.writeFileSync(path.join(disk, 'BUILD.txt'), 'Run node scripts/runtime/prepare-disk-tools.mjs');
  const stale = path.join(directory, 'old-LICENSE');
  fs.writeFileSync(stale, 'old license');
  assert.throws(() => verifyPackagedLicenses(app, pin), /contents/);
  fs.unlinkSync(stale);
  fs.appendFileSync(path.join(disk, 'source.tar.gz'), 'changed');
  assert.throws(() => verifyPackagedLicenses(app, pin), /checksum/);
  fs.unlinkSync(path.join(disk, 'prepare-disk-tools.mjs'));
  assert.throws(() => verifyPackagedLicenses(app, pin), /contents/);
});
