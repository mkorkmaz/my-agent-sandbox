import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('../../src-tauri/src/providers/shared_claude/guest_credentials.py', import.meta.url));
const setup = `
import importlib.util, json, os, stat, sys, tempfile
sys.dont_write_bytecode = True
from pathlib import Path
spec = importlib.util.spec_from_file_location('guest_credentials', ${JSON.stringify(script)})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
`;

function probe(source) {
  const result = spawnSync('python3', ['-c', setup + source], { encoding: 'utf8', timeout: 10_000 });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'passed\n');
}

test('Claude guest updates and sign-out preserve independent connector credentials', () => probe(`
with tempfile.TemporaryDirectory(prefix='tugboat-provider-guest-') as root:
    directory = Path(root)
    file = directory / '.credentials.json'
    connectors = {'connector': {'accessToken': 'synthetic-connector', 'refreshToken': 'independent-refresh'}}
    file.write_text(json.dumps({'mcpOAuth': connectors, 'claudeAiOauth': {'accessToken': 'old'}}))
    incoming = {'claudeAiOauth': {'accessToken': 'synthetic-access', 'refreshToken': None}}
    module.update(directory, 'write', incoming)
    record = json.loads(file.read_text())
    assert record == {'mcpOAuth': connectors, **incoming}
    assert stat.S_IMODE(file.stat().st_mode) == 0o600
    record['claudeAiOauth']['profile'] = {'account': {'uuid': 'synthetic-user'}}
    file.write_text(json.dumps(record))
    modified = file.stat().st_mtime_ns
    module.update(directory, 'write', incoming)
    assert file.stat().st_mtime_ns == modified
    assert json.loads(file.read_text())['claudeAiOauth']['profile'] == record['claudeAiOauth']['profile']
    module.update(directory, 'remove')
    assert json.loads(file.read_text()) == {'mcpOAuth': connectors}
    modified = file.stat().st_mtime_ns
    module.update(directory, 'remove')
    assert file.stat().st_mtime_ns == modified
    file.unlink()
    module.update(directory, 'write', incoming)
    module.update(directory, 'remove')
    assert not file.exists()
print('passed')
`));

test('Claude guest rejects master refresh tokens, malformed files, links, and oversized records without changing them', () => probe(`
with tempfile.TemporaryDirectory(prefix='tugboat-provider-guard-') as root:
    directory = Path(root)
    file = directory / '.credentials.json'
    incoming = {'claudeAiOauth': {'accessToken': 'synthetic-access', 'refreshToken': None}}
    file.write_text('{malformed')
    try:
        module.update(directory, 'write', incoming)
        raise AssertionError('Malformed JSON was overwritten')
    except ValueError:
        assert file.read_text() == '{malformed'
    file.write_text('{}')
    for invalid in [{'claudeAiOauth': {'refreshToken': 'synthetic-master'}}, {'claudeAiOauth': None}]:
        try:
            module.update(directory, 'write', invalid)
            raise AssertionError('A master refresh token reached a guest')
        except ValueError:
            assert file.read_text() == '{}'
    file.unlink()
    target = directory / 'untouched'
    target.write_text('private')
    file.symlink_to(target)
    try:
        module.update(directory, 'write', incoming)
        raise AssertionError('A credential link was followed')
    except OSError:
        assert target.read_text() == 'private'
    file.unlink()
    file.write_text(json.dumps({'connector': 'x' * (module.LIMIT - 30)}))
    original = file.read_bytes()
    try:
        module.update(directory, 'write', incoming)
        raise AssertionError('An oversized combined record was published')
    except ValueError:
        assert file.read_bytes() == original
    assert not list(directory.glob('.tugboat-credentials-*'))
print('passed')
`));
