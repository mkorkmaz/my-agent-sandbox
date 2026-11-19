import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { readProjectGatewayLogs } from '../../scripts/acceptance/read-project-gateway-logs.mjs';

const hiddenEntry = '[Gateway log details removed]';
const timestamp = '2026-11-19T21:54:17+03:00';
const gatewayRecord = message => `time="${timestamp}" level=info msg="${message}" subsystem=hull\n`;
const publishedPort = port => `network-gateway: published 127.0.0.1:${port}=192.168.64.2:23331`;

function fixture(t, output) {
  const root = mkdtempSync(join(tmpdir(), 'tugboat-gateway-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'home'));
  const brig = join(root, 'brig');
  writeFileSync(brig, `#!/bin/sh
test "$#" = 5 && test "$1" = logs && test "$2" = --gateway && test "$4" = --tail && test "$5" = 100 || exit 9
case "$3" in
  preflight@a) ${output} ;;
  preflight@b) printf '%s' '${gatewayRecord(publishedPort(23456))}' ;;
  preflight@empty) printf '\r\n\t' ;;
  preflight@missing) printf 'token=fixture-secret\n' >&2; exit 3 ;;
  *) exit 1 ;;
esac
`, { mode: 0o700 });
  return { root, brig };
}

test('scopes gateway output by ref and only exposes the latest 100 safe lines', async t => {
  const { root, brig } = fixture(t, '/bin/cat "$HOME/sample.log"');
  const output = Array.from({ length: 104 }, (_, index) => gatewayRecord(publishedPort(10000 + index))).join('');
  writeFileSync(join(root, 'home', 'sample.log'), output + [
    gatewayRecord('\x1b[31megress: refused the A query synthetic-host.invalid. from 10.87.0.2\x1b[0m').replace('\n', '\r\n'),
    'OPENAI_API_KEY="synthetic-\x1b[31msecret"\n',
    'GET https://demo.invalid/resource?sample=fixture-query\n',
    'Authorization: Bearer a-synthetic-credential\n',
    'trace gho_syntheticToken\u202e\u0000\n',
  ].join(''));
  const result = await readProjectGatewayLogs(root, 'preflight@a', { brig });
  assert.equal(result.status, 'available');
  const lines = result.text.split('\n');
  assert.equal(lines.length, 100);
  assert.equal(lines[0], `${timestamp} info ${publishedPort(10009)}`);
  assert.equal(lines.at(-5), `${timestamp} info egress: refused a DNS query`);
  assert.deepEqual(lines.slice(-4), Array(4).fill(hiddenEntry));
  assert.deepEqual(await readProjectGatewayLogs(root, 'preflight@b', { brig }), { status: 'available', text: `${timestamp} info ${publishedPort(23456)}` });
});

test('hides multiline credentials after Brig clips the beginning of the block', async t => {
  const { root, brig } = fixture(t, '/bin/cat "$HOME/sample.log"');
  const log = join(root, 'home', 'sample.log');
  const beginning = '-----BEGIN PRIVATE KEY-----\n';
  const end = '-----END PRIVATE KEY-----\n';
  writeFileSync(log, beginning + 'SYNTHETIC-KEY-LINE\n' + end + gatewayRecord(publishedPort(3000)));
  const full = await readProjectGatewayLogs(root, 'preflight@a', { brig });
  assert.deepEqual(full.text.split('\n'), [hiddenEntry, hiddenEntry, hiddenEntry, `${timestamp} info ${publishedPort(3000)}`]);
  writeFileSync(log, 'SYNTHETIC-KEY-CONTINUATION\n' + end + gatewayRecord(publishedPort(3000)));
  const clipped = await readProjectGatewayLogs(root, 'preflight@a', { brig });
  assert.deepEqual(clipped.text.split('\n'), [hiddenEntry, hiddenEntry, `${timestamp} info ${publishedPort(3000)}`]);
});

test('refuses ambiguous references and reports empty, unavailable and failed reads', async t => {
  const { root, brig } = fixture(t, 'exit 1');
  for (const reference of ['', 'preflight', '--raw', 'preflight@a\n', 'preflight@a@b']) {
    await assert.rejects(readProjectGatewayLogs(root, reference, { brig }), /profile@session/);
  }
  assert.deepEqual(await readProjectGatewayLogs(root, 'preflight@empty', { brig }), { status: 'empty' });
  assert.deepEqual(await readProjectGatewayLogs(root, 'preflight@missing', { brig }), { status: 'unavailable' });
  assert.deepEqual(await readProjectGatewayLogs(root, 'preflight@failure', { brig }), { status: 'failed' });
});

test('rejects a scratch home link', async t => {
  const { root, brig } = fixture(t, 'exit 1');
  rmSync(join(root, 'home'), { recursive: true });
  symlinkSync(tmpdir(), join(root, 'home'), 'dir');
  await assert.rejects(readProjectGatewayLogs(root, 'preflight@a', { brig }), /its own home/);
});

test('discards partial and private output at the time and size limits', async t => {
  const stalled = fixture(t, 'printf "token=partial-secret\n"; exec /bin/sleep 10');
  assert.deepEqual(await readProjectGatewayLogs(stalled.root, 'preflight@a', { brig: stalled.brig, timeout: 50 }), { status: 'timeout' });
  const oversized = fixture(t, 'exec /usr/bin/yes "token=oversized-secret"');
  assert.deepEqual(await readProjectGatewayLogs(oversized.root, 'preflight@a', { brig: oversized.brig, timeout: 5_000 }), { status: 'output-limit' });
});
