import assert from 'node:assert/strict';
import { setTimeout } from 'node:timers/promises';

export async function cleanSharedProject(reference, inventory, brig, detach) {
  const sandboxes = await inventory();
  assert.ok(Array.isArray(sandboxes) && sandboxes.length <= 1, 'Unexpected disposable guest inventory.');
  if (sandboxes.length) {
    const [sandbox] = sandboxes;
    assert.equal(sandbox.ref, reference, 'An unrelated guest uses the disposable runtime.');
    assert.ok(['running', 'stopped'].includes(sandbox.state), 'The disposable guest state is unknown.');
    if (sandbox.state === 'running') await brig(['stop', reference]);
    const preview = await brig(['rm', reference, '--dry-run']);
    assert.ok(preview.stdout.includes(reference), 'The removal preview does not identify the disposable guest.');
    await brig(['rm', reference]);
  }
  assert.deepEqual(await inventory(), [], 'A guest remains in the disposable runtime.');
  for (let attempt = 1; attempt <= 3; attempt++) {
    const result = await detach();
    if (result.status === 0 && !result.error && !result.signal) return attempt;
    if (attempt === 3 || result.error || result.signal || !/resource busy|device busy/i.test(result.stderr)) {
      throw new Error('Could not detach the disposable Hull store.');
    }
    await setTimeout(250);
  }
}
