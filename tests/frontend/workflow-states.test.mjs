import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';

function sourceUrl(file) {
  const text = fs.readFileSync(file, 'utf8');
  const code = stripTypeScriptTypes(text).replace(/from ['"](\.{1,2}\/[^'"]+)['"]/g,
    (_match, dependency) => "from '" + sourceUrl(new URL(dependency + '.ts', file)) + "'")
    .replace(/from ['"]zustand\/vanilla['"]/g, "from '" + import.meta.resolve('zustand/vanilla') + "'");
  return 'data:text/javascript;base64,' + Buffer.from(code).toString('base64');
}

async function source(name) { return import(sourceUrl(new URL('../../src/' + name + '.ts', import.meta.url))); }

test('startup admits each step once, rejects overlap, and retries a failed step', async () => {
  const { StartupSequence } = await source('app/startup-state');
  const startup = new StartupSequence();
  let effects = 0;
  await assert.rejects(startup.run('settings', () => effects++));
  await assert.rejects(startup.run('window', () => { throw new Error('fixture'); }));
  let release;
  const first = startup.run('window', () => new Promise(resolve => { release = resolve; }));
  await assert.rejects(startup.run('window', () => effects++));
  release();
  await first;
  for (const step of ['controllers', 'records', 'settings', 'chatgpt', 'claude', 'projects']) {
    await startup.run(step, () => effects++);
  }
  assert.equal(effects, 6);
  await assert.rejects(startup.run('projects', () => effects++));
});

test('dashboard commits require the current project, request, and backend order', async () => {
  const { DashboardState } = await source('dashboard/dashboard-state');
  const dashboard = new DashboardState();
  assert.equal(dashboard.begin(), null);
  dashboard.reset('A');
  const previous = dashboard.begin();
  const event = { sequence: 2, conversations: [] };
  assert.equal(dashboard.event('B', event), false);
  assert.equal(dashboard.event('A', event), true);
  assert.equal(dashboard.snapshot(previous, { conversations: [] }), false);
  assert.equal(dashboard.fail(previous, 'stale'), false);
  const current = dashboard.begin();
  assert.equal(dashboard.snapshot(current, { sequence: 1 }), false);
  assert.equal(dashboard.snapshot(current, { sequence: 3 }), true);
  assert.equal(dashboard.fail(current, 'duplicate'), false);
  dashboard.reset('B');
  dashboard.reset('A');
  assert.equal(dashboard.snapshot(current, { sequence: 99 }), false);
  assert.equal(dashboard.current().sequence, 3);
});

test('usage commits require the active attempt and release only their own pending read', async () => {
  const { UsageDetailsState } = await source('usage/usage-details-state');
  const usage = new UsageDetailsState();
  const details = { sessionId: 'session', agents: [{ sessionId: 'session' }] };
  usage.open('project', 'session', 'environment', 'v1');
  const previous = usage.begin();
  assert.equal(usage.begin(), null);
  usage.refresh('v2');
  assert.equal(usage.show(previous, details), false);
  assert.equal(usage.fail(previous), false);
  usage.finish(previous);
  const current = usage.begin();
  usage.finish(previous);
  assert.equal(usage.begin(), null);
  assert.equal(usage.show(current, details), true);
  assert.equal(usage.fail(current), false);
  usage.close();
  usage.open('project', 'session', 'environment', 'v2');
  assert.equal(usage.show(current, details), false);
  assert.equal(usage.details(), details);
  usage.open('other-project', 'session', 'environment', 'v2');
  assert.equal(usage.details(), null);
});

test('form completion requires the active read or save and reset revokes it', async () => {
  const { FormState } = await source('ui/form-state');
  const form = new FormState();
  form.receive(0, 'unread');
  assert.equal(form.current(), null);
  const old = form.read();
  const current = form.read();
  form.receive(old, 'stale');
  assert.equal(form.current(), null);
  form.receive(current, 'ready');
  form.finish(current);
  const submission = form.submit();
  assert.ok(submission);
  assert.equal(form.submit(), null);
  form.reset();
  form.saved(submission.revision, 'late');
  form.fail(submission.revision);
  assert.equal(form.current(), null);
});

test('cancelled removal and late success cannot reuse another reviewed target', async () => {
  const { RemovalReview } = await source('projects/removal-review');
  const review = new RemovalReview();
  const previous = {}, next = {};
  review.read(1);
  assert.equal(review.review(1, previous, 'previous'), true);
  assert.equal(review.remove('previous'), previous);
  review.reset();
  review.read(2);
  review.review(2, next, 'next');
  review.retry(previous);
  review.completed(previous);
  assert.equal(review.current(), next);
  assert.equal(review.remove('previous'), null);
  assert.equal(review.remove('next'), next);
  review.completed(next);
  assert.equal(review.current(), null);
});

test('sidebar drag retains its pointer until that pointer releases capture', async () => {
  const { SidebarDrag } = await source('sidebar/sidebar-state');
  const drag = new SidebarDrag();
  assert.equal(drag.start(1), true);
  assert.equal(drag.start(2), false);
  assert.equal(drag.end(2), false);
  assert.equal(drag.moves(1), true);
  assert.equal(drag.end(1), true);
  assert.equal(drag.end(1), false);
});

test('terminal layout requests coalesce without overlapping calls and allow retry after failure', async () => {
  const { TerminalLayoutQueue } = await source('terminal/terminal-layout');
  const queue = new TerminalLayoutQueue();
  let release, calls = 0, active = 0;
  const apply = async () => {
    assert.equal(++active, 1);
    calls++;
    if (calls === 1) await new Promise(resolve => { release = resolve; });
    active--;
  };
  const errors = [];
  const work = queue.request(apply, error => errors.push(error));
  assert.equal(queue.request(apply, error => errors.push(error)), work);
  queue.request(apply, error => errors.push(error));
  release();
  await work;
  assert.equal(calls, 2);
  await queue.request(async () => { throw new Error('fixture failure'); }, error => errors.push(error));
  await queue.request(apply, error => errors.push(error));
  assert.equal(calls, 3);
  assert.equal(errors.length, 1);
});

test('terminal launch rejects concurrent starts and duplicate close requests, then permits retry', async () => {
  const { TerminalLaunch } = await source('terminal/terminal-launch');
  const launch = new TerminalLaunch();
  assert.equal(launch.begin(), true);
  assert.equal(launch.begin(), false);
  assert.throws(() => launch.publish());
  launch.start(); launch.publish(); launch.finish();
  assert.equal(launch.begin(), true);
  launch.finish();
  assert.equal(launch.close(7), true);
  assert.equal(launch.close(7), false);
  launch.closed(7);
  assert.equal(launch.close(7), true);
});

test('stale request completion cannot release a newer request or revive an invalidated view', async () => {
  const { RequestState } = await source('ui/request-state');
  const requests = new RequestState();
  const previous = requests.begin(), current = requests.begin();
  requests.finish(previous);
  assert.equal(requests.pending(), true);
  assert.equal(requests.accepts(previous), false);
  requests.finish(current);
  assert.equal(requests.pending(), false);
  assert.equal(requests.accepts(current), true);
  requests.invalidate();
  assert.equal(requests.accepts(current), false);
  requests.finish(current);
  assert.equal(requests.pending(), false);
});


test('recheck an idle request cannot accept a completion with an unissued revision', async () => {
  const {RequestState} = await source('ui/request-state');
  const state = new RequestState();
  state.finish(0);
  assert.equal(state.accepts(0), false);
});
test('recheck a finished callback cannot mark an unstarted scheduler as started', async () => {
  const {ProjectRescan} = await source('projects/project-rescan');
  const state = new ProjectRescan();
  state.finish();
  assert.equal(state.started(), false);
  const initial = state.begin();
  assert.equal(initial.kind, 'initial');
  assert.equal(state.begin(), null);
  state.finish(initial.revision);
  const repeat = state.begin();
  assert.equal(repeat.kind, 'repeat');
  state.finish(initial.revision);
  assert.equal(state.scanning(), true);
  state.finish(repeat.revision);
  assert.equal(state.scanning(), false);
});
test('recheck a late layout request immediately after the apply loop always runs', async () => {
  const {TerminalLayoutQueue} = await source('terminal/terminal-layout');
  const layout = new TerminalLayoutQueue();
  let release;
  const pending = new Promise(resolve => {release = resolve;});
  let calls = 0;
  const apply = async () => {if (++calls === 1) await pending;};
  const work = layout.request(apply, error => {throw error;});
  release(); await Promise.resolve(); await Promise.resolve();
  layout.request(apply, error => {throw error;});
  await work;
  assert.equal(calls, 2);
});
