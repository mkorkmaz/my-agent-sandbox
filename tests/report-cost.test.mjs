import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const html = fs.readFileSync(new URL('../imp-plan.html', import.meta.url), 'utf8');
const start = html.indexOf('function dollarAmount(');
const end = html.indexOf('function validInstant(', start);
assert.ok(start > 0 && end > start);
const context = vm.createContext({
  Intl,
  fmtMoney: new Intl.NumberFormat('tr-TR', { minimumFractionDigits: 2, maximumFractionDigits: 4 }),
  fmtCount: new Intl.NumberFormat('tr-TR'),
});
vm.runInContext(html.slice(start, end), context);

test('comparisons use the unrounded task budgets', () => {
  const estimates = [...html.matchAll(/data-expected-cost="([\d.]+)"/g)].map(match => Number(match[1]));
  assert.equal(estimates.length, 16);
  assert.ok(Math.abs(estimates.reduce((sum, value) => sum + value, 0) - 8.32608) < 1e-9);
  assert.equal(estimates[0], 0.19824);
});

test('the measured cost shows both the excess and the estimate ratio', () => {
  const result = context.comparison([1.4, 1.4], 0.2);
  assert.equal(result.delta, '+1,20 USD');
  assert.equal(result.ratio, '7 kat');
  assert.equal(result.tone, 'cost-over');
});

test('savings and uncertain costs retain their signs and ranges', () => {
  assert.equal(context.comparison([0.1, 0.1], 0.2).delta, '-0,10 USD');
  assert.equal(context.comparison([0.1, 0.1], 0.2).tone, 'cost-under');
  const uncertain = context.comparison([0.1, 0.3], 0.2);
  assert.equal(uncertain.tone, '');
  assert.equal(uncertain.delta, '-0,10–+0,10 USD');
});

test('long-context requests use their full-request multiplier', () => {
  const bounds = context.costBounds({ kind: 'request', input: 300000, read: 0, write: 0, write12: 0, output: 100000 });
  assert.equal(bounds[0], 2.7);
  assert.equal(bounds[1], 2.7);
});

test('token differences use input plus output and preserve the estimate ratio', () => {
  const result = context.tokenComparison(1250000, 250000);
  assert.equal(result.delta, '+1.000.000');
  assert.equal(result.ratio, '5 kat');
  assert.equal(context.tokenComparison(100000, 250000).delta, '-150.000');
});

test('cached percentage uses cached divided by input and keeps missing values unknown', () => {
  assert.equal(context.cachePercent(1000, 600), '%60,0');
  assert.equal(context.cachePercent(1000, 0), '%0,0');
  assert.equal(context.cachePercent(1000, null), '—');
  assert.equal(context.cachePercent(0, 0), '—');
});
