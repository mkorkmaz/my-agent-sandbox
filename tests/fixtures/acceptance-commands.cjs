const { syncBuiltinESMExports } = require('node:module');
const fs = require('node:fs');
const path = require('node:path');
const stage = process.env.TEST_BRIG_STAGE;
const answer = (stdout = '', status = 0, stderr = '') => ({ status, signal: null, stdout, stderr });
const data = value => answer(JSON.stringify({ data: value }));
let guest;
let removed = false;
require('node:child_process').spawnSync = (binary, args, { env, cwd }) => {
  const [verb, sub, ref] = args;
  if (path.basename(binary) === 'hull') return answer('', stage === 'detach' ? 2 : 0);
  if (path.basename(binary) === 'curl') return answer();
  if (verb === '--json') {
    if (sub === 'version') return stage === 'version' ? answer('', 19, 'Brig fixture unavailable') : data({ version: 'v0.5.2' });
    if (sub === 'ls') return removed && stage === 'inventory' ? answer('', 1, 'inventory offline') : data({ sandboxes: guest ? [{ ref: guest }] : [] });
    if (sub === 'plan') return data({ project: { host: cwd }, runtime: { backend: 'hvi', available: true }, credentials: [], verify: { mode: 'require' } });
    if (sub === 'info') return data({ project: { host: cwd }, workspace: path.join(env.BRIG_STATE_DIR, ref) });
  }
  if (verb === 'policy') {
    if (sub === 'create') fs.copyFileSync(env.BRIG_DESKTOP_POLICY_SOURCE, path.join(env.BRIG_POLICY_DIR, ref + '.yaml'));
    return answer();
  }
  if (verb === 'run') { guest = args.find(arg => arg.startsWith('codex@')); return answer('codex fixture'); }
  if (verb === 'stop') return answer();
  if (verb === 'rm') {
    if (args.includes('--dry-run')) return answer(sub);
    removed = true;
    if (stage !== 'remaining') guest = undefined;
    return answer();
  }
  if (verb === 'sh') {
    if (ref === 'uname') return answer('Linux');
    if (ref === 'sh') { fs.writeFileSync(path.join(cwd, 'from-guest.txt'), 'bd15-guest-write'); return answer(); }
    if (ref === 'cat') return answer('', 1);
    if (ref === 'curl') return answer('', args.at(-1) === 'https://example.com' ? 6 : 0);
    if (ref === 'codex') return answer();
  }
  throw new Error('Unexpected fixture command: ' + args.join(' '));
};
syncBuiltinESMExports();
