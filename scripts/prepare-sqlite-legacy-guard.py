#!/usr/bin/env python3
"""Compile the frozen P01 implementation with test-only activated-marker guards."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
manifest = json.loads((root / 'reports/sqlite-usage-evidence/p01/source-manifest.json').read_text())
frozen = Path(manifest['snapshot'])
output = root / 'reports/sqlite-usage-evidence/p06/acceptance'
output.mkdir(parents=True, exist_ok=True)
copy = Path(tempfile.mkdtemp(prefix='tugboat-v1-guard-source-'))
for path in frozen.iterdir():
    if path.name in {'target', 'dist', 'node_modules'}:
        continue
    destination = copy / path.name
    if path.is_symlink():
        destination.symlink_to(path.resolve(), target_is_directory=path.is_dir())
    elif path.is_dir():
        shutil.copytree(path, destination, symlinks=True)
    else:
        shutil.copy2(path, destination)
originals = {}
for relative in ['src-tauri/src/conversations.rs', 'src-tauri/src/data_location.rs', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock']:
    digest = hashlib.sha256((copy / relative).read_bytes()).hexdigest()
    if digest != manifest['files'][relative]:
        raise SystemExit('Frozen baseline source hash differs: ' + relative)
    originals[relative] = digest
with (copy / 'src-tauri/src/data_location.rs').open('a') as stream:
    stream.write(r'''
#[cfg(test)]
pub(crate) fn sqlite_downgrade_fixture(directory: &Path) -> RecordsDirectory {
    RecordsDirectory { directory: fs::canonicalize(directory).unwrap(), id: "a".repeat(32) }
}
#[cfg(test)]
mod sqlite_downgrade_guard {
    use super::*;
    #[test]
    #[ignore = "only used against an isolated generated activated fixture"]
    fn activated_store_is_unavailable_to_the_v1_location_reader() {
        let root = PathBuf::from(std::env::var_os("TUGBOAT_SQLITE_OLD_GUARD_ROOT").unwrap());
        assert_eq!(fs::read(root.join(".acceptance-fixture")).unwrap(), b"synthetic-sqlite-acceptance-v1");
        let location = DataLocation::open(root.join("old-bootstrap"), root.join("old-legacy"));
        assert!(!location.status().unwrap().available);
        assert!(!sqlite_downgrade_fixture(&root.join("records")).verify().is_ok());
    }
}
''')
with (copy / 'src-tauri/src/conversations.rs').open('a') as stream:
    stream.write(r'''
#[cfg(test)]
mod sqlite_downgrade_guard {
    use super::*;
    #[test]
    #[ignore = "only used against an isolated generated activated fixture"]
    fn activated_store_blocks_the_v1_status_writer() {
        let root = PathBuf::from(std::env::var_os("TUGBOAT_SQLITE_OLD_GUARD_ROOT").unwrap());
        assert_eq!(fs::read(root.join(".acceptance-fixture")).unwrap(), b"synthetic-sqlite-acceptance-v1");
        let ledger = root.join("records/conversation-ledgers/11111111111111111111111111111111.json");
        let before = fs::read(&ledger).unwrap();
        let mut lock_name = ledger.as_os_str().to_os_string(); lock_name.push(".lock");
        let had_lock = PathBuf::from(&lock_name).exists();
        let watch = Watch { target: None, records_directory: Some(crate::data_location::sqlite_downgrade_fixture(&root.join("records"))), home: root.join("codex"), project: "/work/p".into(), ledger: ledger.clone() };
        assert!(change_state(&watch, "1a5bc5b5-af21-432d-bf07-e2218dd9f9bf", SessionState::Archived, |_| false).is_err());
        assert!(fs::read(&ledger).unwrap() == before);
        assert_eq!(PathBuf::from(lock_name).exists(), had_lock);
    }
}
''')
environment = dict(os.environ)
environment['CARGO_TARGET_DIR'] = str(frozen / 'target')
command = ['cargo', 'test', '--manifest-path', 'src-tauri/Cargo.toml', '--locked', '--offline', '--no-run', '--message-format=json']
result = subprocess.run(command, cwd=copy, env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
(output / 'old-v1-build.log').write_text(result.stderr)
binaries = []
for line in result.stdout.splitlines():
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        continue
    if event.get('reason') == 'compiler-artifact' and event.get('profile', {}).get('test') and event.get('executable'):
        binaries.append(event['executable'])
record = {'command':command,'exitCode':result.returncode,'snapshotCommit':manifest['commit'],'source':str(copy),'productionSourceHashes':originals,'testOnlyChanges':['appended two cfg(test) guard modules and fixture constructor'],'executables':binaries}
if binaries:
    record['binarySha256'] = hashlib.sha256(Path(binaries[-1]).read_bytes()).hexdigest()
    (output / 'old-v1-binary.txt').write_text(binaries[-1] + '\n')
(output / 'old-v1-build.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record, indent=2))
raise SystemExit(result.returncode)
