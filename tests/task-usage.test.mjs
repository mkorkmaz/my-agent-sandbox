import assert from 'node:assert/strict';
import test from 'node:test';
import { collectUsage, normalizeUsage } from '../scripts/record-task-usage.mjs';

const counters = (input, read, write, output, reasoning = 0) => ({
  input_tokens: input, cached_input_tokens: read, cache_write_input_tokens: write,
  output_tokens: output, reasoning_output_tokens: reasoning, total_tokens: input + output,
});
const measurement = { turnId: 'implementation', model: 'gpt-6.1-sol', startedAt: '2026-11-10T10:00:00Z', baseline: { usage: counters(10000, 2000, 0, 500) } };
const event = (type, payload = {}, timestamp = '2026-11-10T10:01:00Z') => ({ type: 'event_msg', timestamp, payload: { type, ...payload } });
const start = [event('task_started', { turn_id: measurement.turnId }), { type: 'turn_context', payload: { model: measurement.model } }];
const usage = (total, last, time) => event('token_count', { info: { total_token_usage: total, last_token_usage: last } }, time);

test('reasoning already included in output is counted once', () => {
  assert.equal(normalizeUsage(counters(100, 40, 0, 30, 10)).output, 30);
});

test('separately reported reasoning is included once', () => {
  const split = counters(100, 40, 0, 30, 10);
  split.total_tokens += 10;
  assert.equal(normalizeUsage(split).output, 40);
});

test('duplicate cumulative events are ignored and another turn is excluded', () => {
  const sample = usage(counters(11000, 2400, 0, 530), counters(1000, 400, 0, 30));
  const rows = [...start, sample, sample, event('task_complete', { turn_id: measurement.turnId }),
    event('task_started', { turn_id: 'other' }), usage(counters(12000, 2400, 0, 560), counters(1000, 0, 0, 30))];
  const result = collectUsage(rows, measurement);
  assert.equal(result.calls.length, 1);
  assert.equal(result.calls[0].input, 1000);
  assert.equal(result.calls[0].kind, 'request');
  assert.equal(result.complete, true);
  assert.deepEqual(result.anomalies, []);
});

test('a cumulative difference spanning requests retains window pricing', () => {
  const rows = [...start, usage(counters(310000, 2400, 0, 560), counters(1000, 0, 0, 30))];
  const result = collectUsage(rows, measurement);
  assert.equal(result.calls[0].input, 300000);
  assert.equal(result.calls[0].kind, 'window');
});

test('counter reset preserves the latest request and marks incomplete coverage', () => {
  const rows = [...start, usage(counters(1000, 400, 0, 30), counters(1000, 400, 0, 30))];
  const result = collectUsage(rows, measurement);
  assert.equal(result.calls[0].input, 1000);
  assert.equal(result.anomalies.length, 1);
});

test('cache write retention stays unknown when the source does not report it', () => {
  const rows = [...start, usage(counters(11000, 2400, 50, 530), counters(1000, 400, 50, 30))];
  const result = collectUsage(rows, measurement);
  assert.equal(result.calls[0].write, 50);
  assert.equal(result.calls[0].write12, null);
});

test('a model change fails attribution', () => {
  assert.throws(() => collectUsage([...start, { type: 'turn_context', payload: { model: 'another-model' } }], measurement), /Model changed/);
});

test('consecutive tasks in one turn have disjoint counter windows', () => {
  const firstTime = '2026-11-10T10:01:00Z';
  const secondTime = '2026-11-10T10:02:00Z';
  const firstTotal = counters(11000, 2400, 0, 530);
  const rows = [...start,
    usage(firstTotal, counters(1000, 400, 0, 30), firstTime),
    usage(counters(13000, 2900, 0, 590), counters(2000, 500, 0, 60), secondTime)];
  const first = collectUsage(rows, { ...measurement, usageEndedAt: firstTime });
  const second = collectUsage(rows, { ...measurement, baseline: { at: firstTime, usage: firstTotal }, usageEndedAt: secondTime });
  assert.equal(first.calls.length, 1);
  assert.equal(second.calls.length, 1);
  assert.equal(first.complete, true);
  assert.equal(second.complete, true);
  assert.equal(first.calls[0].input + second.calls[0].input, 3000);
  assert.notEqual(first.calls[0].key, second.calls[0].key);
});
