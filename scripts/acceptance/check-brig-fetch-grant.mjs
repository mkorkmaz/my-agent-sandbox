import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runAcceptanceProcess } from './acceptance-process.mjs';

const workspace = fileURLToPath(new URL('../../', import.meta.url));
const readJson = path => JSON.parse(readFileSync(join(workspace, path), 'utf8'));
const pin = readJson('config/brig-compat.json');
const prepared = readJson('.build/brig-compat/prepared.json');
if (prepared.commit !== pin.commit || prepared.version !== pin.version || prepared.source.sha256 !== pin.source.sha256) {
  throw new Error('Prepare the pinned Brig runtime before running this check.');
}

const source = join(workspace, '.build/brig-compat', `brig-${pin.commit}`);
const goWorkspace = join(workspace, '.build/hull-compat');
const scratch = mkdtempSync(join(tmpdir(), 'tugboat-brig-fetch-'));
try {
  const overlay = join(scratch, 'overlay.json');
  writeFileSync(overlay, JSON.stringify({ Replace: {
    [join(source, 'internal/broker/desktop_fetch_grant_test.go')]: join(workspace, 'tests/native/brig_fetch_grant_test.go'),
  } }));
  const result = await runAcceptanceProcess(join(goWorkspace, 'go/bin/go'), [
    'test', '-mod=readonly', '-count=1', '-overlay', overlay,
    '-run', '^TestTugboatProjectFetchGrant$', '-v', './internal/broker',
  ], {
    cwd: source, timeout: 120_000,
    env: {
      HOME: scratch, TMPDIR: scratch, PATH: '/usr/bin:/bin:/usr/sbin:/sbin',
      GOCACHE: join(scratch, 'go-cache'), GOMODCACHE: join(goWorkspace, 'go-path/pkg/mod'),
      CGO_ENABLED: '0', GOTOOLCHAIN: 'local', GOPROXY: 'off', GOSUMDB: 'off',
      TUGBOAT_FETCH_GRANT_FIXTURE: join(workspace, `tests/fixtures/brig/${pin.version}/github-fetch-policy.yaml`),
    },
  });
  process.stdout.write(result.stdout);
  process.stderr.write(result.stderr);
  if (result.error || result.status !== 0) process.exitCode = 1;
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
