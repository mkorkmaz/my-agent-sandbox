#!/usr/bin/env python3
"""Compare complete payloads from frozen P01 and final SQL Rust builds.

Both workers use the same generated native files, paths and inodes. Includes
are injected only into disposable source copies. No app or account is used.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "reports/sqlite-usage-evidence/p06/acceptance/frozen-build-parity"
TEST = "conversations::environment_history_tests::sqlite_frozen_full_contract_export"
CASES = ["codex-family", "claude-main", "priced-subagents", "subagents"]
ADAPTER = ROOT / "tests/fixtures/sqlite-usage/frozen-full-export.rs"

LEGACY = r'''
fn frozen_prepare(_watch: &mut Watch, _base: &Path, _name: &str) {}
fn frozen_read(watch: &Watch) -> Ledger { load_ledger(&watch.ledger).unwrap() }
fn frozen_details(_watch: &Watch, root: &str, ledger: &Ledger) -> Value {
    serde_json::to_value(usage_details::from_ledger(ledger, root).unwrap()).unwrap()
}
fn frozen_finish(_watch: &Watch) {}
'''

SQL = r'''
fn frozen_prepare(watch: &mut Watch, base: &Path, name: &str) {
    let directory = base.join(name).join("records");
    fs::create_dir(&directory).unwrap();
    let records = crate::storage::data_location::RecordsDirectory::fixture(&directory);
    let target = watch.target.as_ref().unwrap();
    let source = base.join("inputs").join(name).join("ledger.json");
    let ledger = if target.agent_kind == AgentKind::Codex {
        directory.join("conversation-ledgers").join(format!("{PROJECT}.json"))
    } else {
        directory.join("conversation-ledgers").join(PROJECT).join(format!("{}.json",target.environment_id))
    };
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    fs::copy(&source,&ledger).unwrap();
    crate::usage_store::importer::activate(&records).unwrap();
    assert_eq!(fs::read(&source).unwrap(),fs::read(&ledger).unwrap());
    watch.records_directory=Some(records.clone());
    watch.store=Some(crate::usage_store::SharedStore::new(crate::usage_store::Store::open(&records).unwrap()));
    watch.ledger=ledger;
}
fn frozen_read(watch: &Watch) -> Ledger { watch.load().unwrap() }
fn frozen_details(watch: &Watch, root: &str, _ledger: &Ledger) -> Value {
    serde_json::to_value(sql_backend::details(watch.target.as_ref().unwrap(),watch.store.as_ref().unwrap(),root,|_|Ok(Some(watch.namespace()))).unwrap()).unwrap()
}
fn frozen_finish(watch: &Watch) {
    watch.store.as_ref().unwrap().close().unwrap();
    let base=PathBuf::from(std::env::var_os("TUGBOAT_SQLITE_FROZEN_ROOT").unwrap());
    let source=base.join("inputs").join(watch.home.parent().unwrap().file_name().unwrap()).join("ledger.json");
    assert_eq!(fs::read(source).unwrap(),fs::read(&watch.ledger).unwrap());
}
'''


def write(name, value):
    (EVIDENCE / name).write_text(json.dumps(value, indent=2) + "\n")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_copy(source, target, adapter):
    ignored = shutil.ignore_patterns(".git", ".build", "target", "node_modules", "dist", "reports",
                                     "artifacts", ".impeccable", ".aws", ".codex", ".agents")
    shutil.copytree(source, target, symlinks=True, ignore=ignored)
    (target / ".build").symlink_to((source / ".build").resolve(), target_is_directory=True)
    tests = target / "src-tauri/src/conversations"
    parent = tests / ("environment_history_tests.rs" if adapter == LEGACY else "environment_history_tests/mod.rs")
    module = parent.with_name("frozen_full_export.rs")
    module.write_text(ADAPTER.read_text().replace("// FROZEN_BACKEND_ADAPTER", adapter))
    with parent.open("a") as stream:
        stream.write('\ninclude!("frozen_full_export.rs");\n')


def clone_debug_cache(cache, target):
    target.mkdir()
    command = ["cp", "-cR", str(cache / "debug"), str(target / "debug")]
    subprocess.run(command, check=True, timeout=120)
    for name in [".rustc_info.json", "CACHEDIR.TAG"]:
        if (cache / name).exists():
            shutil.copy2(cache / name, target / name)


def build(source, target, backend):
    command = ["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "--locked", "--offline", "--no-run", "--message-format=json"]
    environment = dict(os.environ, CARGO_TARGET_DIR=str(target))
    start = time.monotonic()
    result = subprocess.run(command, cwd=source, env=environment, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            timeout=600, check=False)
    (EVIDENCE / f"build-{backend}.log").write_text(result.stdout)
    artifacts = []
    for line in result.stdout.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("reason") == "compiler-artifact" and event.get("executable") and event.get("profile", {}).get("test"):
            artifacts.append(event)
    if result.returncode or len(artifacts) != 1:
        raise RuntimeError(f"The {backend} test build failed; inspect its log.")
    artifact = artifacts[0]
    if artifact["fresh"]:
        raise RuntimeError("The injected adapter did not cause a fresh owning-crate compilation.")
    binary = Path(artifact["executable"])
    record = {"command": command, "source": str(source), "target": str(target),
              "exitCode": result.returncode, "seconds": time.monotonic() - start,
              "owningCrateFresh": artifact["fresh"], "binary": str(binary), "binarySha256": digest(binary)}
    write(f"build-{backend}.json", record)
    return binary, record


def native_files(base):
    result = {}
    for case in CASES:
        for agent in ["codex", "claude"]:
            home = base / case / agent
            if not home.exists():
                continue
            for path in sorted(home.rglob("*")):
                if path.is_file():
                    stat = path.stat()
                    result[str(path.relative_to(base))] = {"sha256": digest(path), "device": stat.st_dev,
                        "inode": stat.st_ino, "bytes": stat.st_size, "modifiedNs": stat.st_mtime_ns}
    return result


def worker(binary, base, backend):
    output = EVIDENCE / backend
    output.mkdir(exist_ok=True)
    environment = dict(os.environ, TUGBOAT_SQLITE_FROZEN_ROOT=str(base),
                       TUGBOAT_SQLITE_FROZEN_OUTPUT=str(output), TUGBOAT_SQLITE_FROZEN_BACKEND=backend)
    command = [str(binary), "--ignored", "--exact", TEST, "--test-threads=1"]
    result = subprocess.run(command, cwd=ROOT, env=environment, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=180, check=False)
    (EVIDENCE / f"run-{backend}.log").write_text(result.stdout)
    write(f"run-{backend}.json", {"command": command, "exitCode": result.returncode})
    if result.returncode or "1 passed" not in result.stdout:
        raise RuntimeError(f"The {backend} full contract worker failed; inspect its log.")


def differences(left, right, path="$"):
    if type(left) is not type(right):
        return [{"path": path, "json": left, "sql": right}]
    if isinstance(left, dict):
        if left.keys() != right.keys():
            return [{"path": path, "jsonKeys": list(left), "sqlKeys": list(right)}]
        return [entry for key in left for entry in differences(left[key], right[key], path + "." + key)]
    if isinstance(left, list):
        if len(left) != len(right):
            return [{"path": path, "jsonLength": len(left), "sqlLength": len(right)}]
        return [entry for index, (a, b) in enumerate(zip(left, right)) for entry in differences(a, b, f"{path}[{index}]")]
    return [] if left == right else [{"path": path, "json": left, "sql": right}]


def main():
    global EVIDENCE
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sql-cache", type=Path, required=True, help="An owned Cargo target directory with a debug subdirectory")
    parser.add_argument("--output", type=Path, default=EVIDENCE)
    arguments = parser.parse_args()
    EVIDENCE = arguments.output.resolve()
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    manifest = json.loads((ROOT / "reports/sqlite-usage-evidence/p01/source-manifest.json").read_text())
    frozen = Path(manifest["snapshot"])
    verified = {}
    for relative, expected in manifest["files"].items():
        path = Path(relative)
        production = relative.startswith("src-tauri/src/") and path.suffix == ".rs" and not (path.name == "tests.rs" or path.name.endswith("_tests.rs"))
        if production or relative in ["src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/build.rs"]:
            actual = digest(frozen / relative)
            if actual != expected:
                raise RuntimeError("Frozen owning production source differs: " + relative)
            verified[relative] = actual
    write("frozen-source.json", {"commit": manifest["commit"], "verifiedProductionFiles": verified,
          "testOnlyChange": "Injected shared export adapter in disposable source copies only"})
    work = Path(tempfile.mkdtemp(prefix="tugboat-frozen-full-contract-"))
    base = work / "fixtures"
    base.mkdir()
    (base / ".frozen-contract-fixture").write_bytes(b"generated-full-sqlite-contract-v1")
    record = {"status": "failed", "generatedRoot": str(base), "normalization": "Only transport sequence is fixed at1; no paths, namespaces, hashes, values or ordering are normalized."}
    try:
        source_copy(frozen, work / "source-json", LEGACY)
        source_copy(ROOT, work / "source-sql", SQL)
        sql_sources = [path for path in (ROOT / "src-tauri/src").rglob("*.rs")
                       if path.name != "tests.rs" and not path.name.endswith("_tests.rs")]
        sql_sources += [ROOT / path for path in ["src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/build.rs", "src-tauri/src/usage_store/migrations/001_initial.sql"]]
        sql_hashes = {str(path.relative_to(ROOT)): digest(path) for path in sql_sources}
        write("sql-source.json", sql_hashes)
        clone_debug_cache(frozen / "target", work / "target-json")
        clone_debug_cache(arguments.sql_cache.resolve(), work / "target-sql")
        old, old_build = build(work / "source-json", work / "target-json", "json")
        current, sql_build = build(work / "source-sql", work / "target-sql", "sql")
        worker(old, base, "json")
        native_before = native_files(base)
        write("native-before.json", native_before)
        legacy_before = {case: digest(base / case / ("codex-ledger.json" if case == "codex-family" else "claude-ledger.json")) for case in CASES}
        worker(current, base, "sql")
        native_after = native_files(base)
        write("native-after.json", native_after)
        if native_before != native_after:
            raise RuntimeError("Native fixture bytes, paths, inodes or times changed between builds.")
        for case in CASES:
            legacy = base / case / ("codex-ledger.json" if case == "codex-family" else "claude-ledger.json")
            if digest(legacy) != legacy_before[case]:
                raise RuntimeError("The SQL writer changed the original generated ledger.")
        results = []
        for case in CASES:
            left_path = EVIDENCE / "json" / case / "trace.json"
            right_path = EVIDENCE / "sql" / case / "trace.json"
            mismatches = differences(json.loads(left_path.read_text()), json.loads(right_path.read_text()))
            results.append({"case": case, "status": "failed" if mismatches else "passed", "mismatches": mismatches,
                "jsonSha256": digest(left_path), "sqlSha256": digest(right_path)})
        record.update({"status": "passed" if all(row["status"] == "passed" for row in results) else "failed",
            "cases": results, "nativeFilesUnchanged": True, "originalLedgersUnchanged": True,
            "builds": {"json": old_build, "sql": sql_build}, "verifiedP01ProductionFileCount": len(verified)})
    except Exception as failure:
        record["error"] = str(failure)
        raise
    finally:
        shutil.rmtree(work)
        record["generatedWorkRemoved"] = not work.exists()
        write("results.json", record)
    print(json.dumps(record, indent=2))
    return int(record["status"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())
