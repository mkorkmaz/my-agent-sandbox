import assert from 'node:assert/strict';
import { test } from 'node:test';
import { deferred, flush, interfaceHarness } from './frontend-harness.mjs';

const installedVersion = '0.4.1';
const chat = { kind: 'chat', label: 'Tugboat · Codex · Saved chat' };
const terminal = { kind: 'terminal', label: 'Tools · Local terminal' };
const offer = {
  id: 42, version: '0.4.2', notes: 'Release notes with <b>literal text</b>.',
  sessionRevision: 1, sessions: [chat],
};
const idle = { revision: 0, phase: 'idle', installedVersion };

async function setup(t, handlers) {
  const harness = await interfaceHarness({ app_update_status: () => idle, ...handlers });
  t.after(() => harness.dom.window.close());
  await harness.ready;
  return harness;
}

async function click(harness, id) {
  harness.element(id).click();
  await flush();
}

async function update(harness, status) {
  await harness.listeners.get('app-update')({ payload: { ...idle, ...status } });
  await flush();
}

function actions(harness, command) {
  return harness.calls.filter(call => call.command === command).map(call => ({ ...call.args }));
}

test('an update status read does not delay startup and hides production controls in Tugboat Dev', async t => {
  const pending = deferred();
  const harness = await setup(t, { app_update_status: () => pending.promise });
  assert.ok(harness.calls.some(call => call.command === 'rescan_projects'));
  assert.equal(harness.element('app-updates').hidden, true);
  pending.resolve({ ...idle, phase: 'disabled' });
  await flush();
  await harness.listeners.get('app-update-menu')({});
  assert.equal(harness.element('app-updates').hidden, true);
  assert.equal(harness.element('app-update-dialog').open, false);
  assert.deepEqual(actions(harness, 'app_update_check'), []);
});

test('an automatic offer shows the release and sessions, and Later keeps it in Settings', async t => {
  let state = idle;
  const harness = await setup(t, {
    app_update_status: () => state,
    app_update_later: ({ offerId }) => {
      assert.equal(offerId, offer.id);
      state = { ...state, revision: 2, automaticOffer: false };
      return state;
    },
  });
  await click(harness, 'choose-folder');
  await click(harness, 'start-codex');
  assert.equal(harness.terminals.get(1).running, true);
  const layout = () => harness.calls.filter(call => call.command === 'terminal_layout').at(-1).args.layout.visible;
  assert.equal(layout(), true);
  state = { ...idle, revision: 1, phase: 'available', offer, automaticOffer: true };
  await update(harness, state);
  const dialog = harness.element('app-update-dialog');
  assert.equal(dialog.open, true);
  assert.equal(layout(), false);
  assert.equal(harness.element('app-update-installed').textContent, installedVersion);
  assert.equal(harness.element('app-update-version').textContent, offer.version);
  assert.equal(harness.element('app-update-notes').textContent, offer.notes);
  assert.equal(harness.element('app-update-notes').querySelector('b'), null);
  assert.match(harness.element('app-update-impact').textContent, /stop its Brig guests.*saved chats stay in place/);
  assert.match(harness.element('app-update-sessions').textContent, /Chat · Tugboat · Codex · Saved chat/);
  assert.equal(harness.element('approve-app-update').textContent, 'End Sessions, Update and Restart');
  assert.equal(harness.element('dismiss-app-update').textContent, 'Later');
  dialog.dispatchEvent(new harness.dom.window.Event('cancel', { cancelable: true }));
  await flush();
  assert.equal(dialog.open, false);
  assert.equal(layout(), true);
  assert.equal(harness.terminals.get(1).running, true);
  assert.deepEqual(actions(harness, 'terminal_focus').at(-1), { identifier: 1 });
  assert.deepEqual(actions(harness, 'terminal_close'), []);
  assert.deepEqual(actions(harness, 'app_update_later'), [{ offerId: offer.id }]);
  assert.deepEqual(actions(harness, 'app_update_check'), []);
  assert.deepEqual(actions(harness, 'app_update_approve'), []);

  await click(harness, 'settings-navigation');
  assert.equal(harness.element('review-app-update').hidden, false);
  await click(harness, 'review-app-update');
  assert.equal(dialog.open, true);
  await click(harness, 'dismiss-app-update');
  assert.equal(harness.dom.window.document.activeElement.id, 'review-app-update');
});

test('a manual check shows progress and reports current or offline from Settings and the menu', async t => {
  const pending = deferred();
  let count = 0;
  const harness = await setup(t, {
    app_update_check: () => ++count === 1 ? pending.promise : {
      ...idle, revision: 2, phase: 'failed', message: 'Could not check for updates. Check the Mac connection.',
    },
  });
  await click(harness, 'settings-navigation');
  await click(harness, 'check-app-updates');
  assert.equal(harness.element('app-update-dialog').open, true);
  assert.equal(harness.element('app-update-status').textContent, 'Checking for updates…');
  assert.equal(harness.element('approve-app-update').hidden, true);
  pending.resolve({ ...idle, revision: 1, phase: 'current' });
  await flush();
  assert.equal(harness.element('app-update-status').textContent, 'Tugboat is up to date.');
  await click(harness, 'dismiss-app-update');
  assert.equal(harness.dom.window.document.activeElement.id, 'check-app-updates');

  await harness.listeners.get('app-update-menu')({});
  await flush();
  assert.equal(harness.element('app-update-dialog').open, true);
  assert.match(harness.element('app-updates-status').textContent, /Could not check.*Mac connection/);
  assert.equal(harness.element('approve-app-update').hidden, true);
  assert.equal(actions(harness, 'app_update_check').length, 2);
  assert.deepEqual(actions(harness, 'app_update_approve'), []);
});

test('closing a pending manual check lets a chat start before the network check finishes', async t => {
  const pending = deferred();
  const harness = await setup(t, { app_update_check: () => pending.promise });
  await harness.listeners.get('app-update-menu')({});
  await click(harness, 'dismiss-app-update');
  await click(harness, 'choose-folder');
  await click(harness, 'start-codex');
  assert.equal(harness.terminals.get(1).running, true);
  pending.resolve({ ...idle, revision: 1, phase: 'failed', message: 'Could not check for updates.' });
  await flush();
  assert.equal(harness.element('app-update-dialog').open, false);
  assert.equal(harness.terminals.get(1).running, true);
});

test('a rejected download requires a fresh check and approval of the current release', async t => {
  const replacement = { ...offer, id: 45, version: '0.4.3' };
  const harness = await setup(t, {
    app_update_check: () => ({ ...idle, revision: 4, phase: 'available', offer: replacement }),
    app_update_approve: () => ({ ...idle, revision: 5, phase: 'downloading', offer: replacement }),
  });
  await update(harness, { revision: 1, phase: 'available', offer, automaticOffer: true });
  await update(harness, { revision: 2, phase: 'downloading', offer });
  await update(harness, { revision: 3, phase: 'failed', message: 'The update signature could not be verified.' });
  assert.equal(harness.element('approve-app-update').hidden, true);
  assert.equal(harness.element('recheck-app-update').hidden, false);
  await click(harness, 'recheck-app-update');
  assert.deepEqual(actions(harness, 'app_update_check'), [{}]);
  assert.deepEqual(actions(harness, 'app_update_approve'), []);
  assert.equal(harness.element('app-update-version').textContent, replacement.version);
  await click(harness, 'approve-app-update');
  assert.deepEqual(actions(harness, 'app_update_approve'), [{ offerId: replacement.id, sessionRevision: replacement.sessionRevision }]);
});

test('repeated approval sends one command and a changed session list needs a new approval', async t => {
  const pending = deferred();
  const harness = await setup(t, {
    app_update_approve: ({ sessionRevision }) => sessionRevision === 1 ? pending.promise : {
      ...idle, revision: 6, phase: 'shuttingDown', offer: { ...offer, sessionRevision, sessions: [chat, terminal] },
    },
  });
  const original = { ...idle, revision: 1, phase: 'available', offer, automaticOffer: true };
  await update(harness, original);
  harness.element('approve-app-update').click();
  await click(harness, 'approve-app-update');
  assert.deepEqual(actions(harness, 'app_update_approve'), [{ offerId: offer.id, sessionRevision: 1 }]);

  await update(harness, { revision: 2, phase: 'downloading', offer, progress: { downloaded: 20, total: 50 } });
  const progress = harness.element('app-update-progress');
  assert.equal(progress.hidden, false);
  assert.equal(progress.position, 0.4);
  await click(harness, 'dismiss-app-update');
  assert.equal(harness.element('app-update-dialog').open, false);
  assert.deepEqual(actions(harness, 'app_update_later'), []);
  await update(harness, { revision: 3, phase: 'ready', offer: { ...offer, sessionRevision: 2, sessions: [chat, terminal] }, message: 'A new session started. Review the sessions again.' });
  pending.resolve({ ...idle, revision: 2, phase: 'downloading', offer });
  await flush();
  assert.equal(progress.hidden, true);
  assert.equal(harness.element('app-update-dialog').open, true);
  assert.match(harness.element('app-update-sessions').textContent, /Terminal · Tools · Local terminal/);
  assert.match(harness.element('app-update-status').textContent, /new session started/);
  await click(harness, 'approve-app-update');
  assert.deepEqual(actions(harness, 'app_update_approve'), [{ offerId: offer.id, sessionRevision: 1 }, { offerId: offer.id, sessionRevision: 2 }]);
  assert.equal(harness.element('approve-app-update').hidden, true);
  assert.equal(harness.element('dismiss-app-update').hidden, true);
});

test('read-only, shutdown, and relaunch failures offer their matching recovery actions', async t => {
  const harness = await setup(t, {
    app_update_open_installer: () => null,
    app_update_reopen: () => { throw 'Could not reopen Tugboat 0.4.2.'; },
    app_update_approve: ({ sessionRevision }) => ({ ...idle, revision: 4, phase: 'shuttingDown', offer: { ...offer, sessionRevision } }),
  });
  await update(harness, { revision: 1, phase: 'available', offer, automaticOffer: true });
  await update(harness, { revision: 2, phase: 'failed', offer, message: 'This copy is read-only. Install Tugboat in Applications.', installerUrl: 'https://github.com/reformo/tugboat/releases/latest' });
  assert.equal(harness.element('approve-app-update').hidden, true);
  assert.match(harness.element('app-update-status').textContent, /read-only.*Applications/);
  await click(harness, 'open-app-update-installer');
  assert.equal(actions(harness, 'app_update_open_installer').length, 1);

  await update(harness, { revision: 3, phase: 'ready', offer, message: 'Could not stop the registered Brig guest. Try installing again.' });
  assert.match(harness.element('app-update-status').textContent, /Could not stop the registered Brig guest/);
  assert.equal(harness.element('approve-app-update').disabled, false);
  await click(harness, 'approve-app-update');
  assert.deepEqual(actions(harness, 'app_update_approve'), [{ offerId: offer.id, sessionRevision: 1 }]);

  await update(harness, { revision: 5, phase: 'recovery', installedVersion: offer.version, canReopen: true, message: 'Could not restart Tugboat.' });
  assert.match(harness.element('app-update-status').textContent, /0\.4\.2 is installed/);
  assert.equal(harness.element('approve-app-update').hidden, true);
  await click(harness, 'reopen-app');
  assert.match(harness.element('app-update-status').textContent, /Could not reopen Tugboat 0\.4\.2/);
  assert.equal(harness.element('reopen-app').disabled, false);
  assert.equal(actions(harness, 'app_update_reopen').length, 1);
});
