import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { JSDOM, VirtualConsole } from 'jsdom';

const html = fs.readFileSync(new URL('../imp-plan.html', import.meta.url), 'utf8');
const ledgerPattern = /(<script type="application\/json" id="agent-ledger">)([\s\S]*?)(<\/script>)/;
const original = JSON.parse(html.match(ledgerPattern)[2]);
const count = new Intl.NumberFormat('tr-TR');

function report(book = original) {
  const errors = [];
  const virtualConsole = new VirtualConsole();
  virtualConsole.on('jsdomError', error => errors.push(error.message));
  const source = html.replace(ledgerPattern, (_, start, old, end) => start + JSON.stringify(book) + end);
  const dom = new JSDOM(source, { runScripts: 'dangerously', virtualConsole });
  assert.deepEqual(errors, []);
  assert.ok(!dom.window.document.getElementById('ledger-message').textContent.includes('okunamadı'));
  return dom;
}

function breakdown(container) {
  return Object.fromEntries([...container.querySelectorAll('.token-breakdown > div')].map(row => [
    row.querySelector('dt').textContent,
    row.querySelector('dd').firstChild.textContent,
  ]));
}

test('totals and all 16 task comparisons reconcile with the original counters', () => {
  const dom = report();
  const d = dom.window.document;
  const calls = Object.values(original.items).flatMap(item => item.usage);
  const sum = field => calls.reduce((total, call) => total + call[field], 0);
  const total = breakdown(d.getElementById('total-token-breakdown'));
  assert.equal(total.Input, count.format(sum('input')));
  assert.equal(total.Cached, count.format(sum('read')));
  assert.equal(total.Output, count.format(sum('output')));
  assert.equal(d.getElementById('spent-tokens').textContent, count.format(sum('input') + sum('output')) + '*');
  assert.equal(d.getElementById('estimated-tokens').textContent, '6.300.000');
  assert.equal(d.querySelectorAll('#ledger-rows tr').length, 16);
  for (const [id, item] of Object.entries(original.items)) {
    const task = d.getElementById(id);
    const input = item.usage.reduce((total, call) => total + call.input, 0);
    const output = item.usage.reduce((total, call) => total + call.output, 0);
    const mark = item.coverage === 'partial' ? '*' : '';
    assert.equal(task.querySelector('.token-measured').textContent, count.format(input + output) + mark);
    assert.equal(breakdown(task.querySelector('.record-comparison')).Input, count.format(input));
    const row = [...d.querySelectorAll('#ledger-rows tr')].find(row => row.cells[0].querySelector('a').textContent === id);
    assert.equal(row.cells.length, 8);
    assert.equal(row.cells[2].firstChild.textContent, count.format(input + output) + mark);
  }
  dom.window.close();
});

test('missing cache counters and unmeasured work are never presented as zero usage', () => {
  const partial = structuredClone(original);
  partial.items['BD-01'].coverage = 'partial';
  partial.items['BD-01'].usage = [{ key: 'fixture/partial', kind: 'request', input: 1000, read: null, write: 0, write12: 0, output: 200 }];
  const first = report(partial);
  assert.equal(breakdown(first.window.document.querySelector('#BD-01 .record-comparison')).Cached, 'Ayrım eksik');
  assert.equal(first.window.document.querySelector('#BD-01 .cache-percentage').textContent, 'Input oranı: —');
  first.window.close();
  const empty = structuredClone(original);
  empty.asOf = null;
  for (const item of Object.values(empty.items)) Object.assign(item, { state: 'planned', coverage: 'none', log: [], usage: [], periods: [] });
  const second = report(empty);
  assert.equal(second.window.document.getElementById('spent-tokens').textContent, '—');
  assert.deepEqual(breakdown(second.window.document.querySelector('#BD-01 .record-comparison')), { Input: '—', Cached: '—', Output: '—' });
  second.window.close();
});

test('printing expands token details and restores the previous disclosure states', () => {
  const dom = report();
  const details = [...dom.window.document.querySelectorAll('.token-detail')];
  const before = details.map(detail => detail.open);
  assert.ok(before.some(open => !open));
  dom.window.dispatchEvent(new dom.window.Event('beforeprint'));
  assert.ok(details.every(detail => detail.open));
  dom.window.dispatchEvent(new dom.window.Event('afterprint'));
  assert.deepEqual(details.map(detail => detail.open), before);
  dom.window.close();
});
