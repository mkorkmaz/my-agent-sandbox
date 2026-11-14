#!/usr/bin/env python3
"""Run the new two-read assertion against the archived earlier append writer.

The worker uses only generated Rust fixtures. Restore two earlier production
files in a separate source copy. Keep all current production files unchanged.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
BASE = ROOT / "reports/sqlite-usage-evidence/p06/append-remediation"
TEST = "conversations::environment_history_tests::sqlite_usage_tests::acceptance::append::appending_one_record_preserves_unrelated_facts_and_atomically_saves_source_loss"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, required=True)
    arguments = parser.parse_args()
    output = BASE / "acceptance/fail-before"
    output.mkdir(parents=True, exist_ok=True)
    specification = importlib.util.spec_from_file_location("frozen_helpers", ROOT / "scripts/compare-sqlite-frozen-contracts.py")
    helpers = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(helpers)
    helpers.EVIDENCE = output
    manifest = json.loads((BASE / "b494-source.json").read_text())
    work = Path(tempfile.mkdtemp(prefix="tugboat-append-read-probe-"))
    record = {"status": "failed", "archivedReferenceBinarySha256": manifest["binarySha256"],
              "generatedDataOnly": True, "test": TEST}
    try:
        source = work / "source"
        helpers.source_copy(ROOT, source, helpers.SQL)
        restored = {}
        for relative in ["src-tauri/src/conversations/sql_scan.rs", "src-tauri/src/usage_store/writes.rs"]:
            archived = BASE / "b494-source" / relative
            digest = hashlib.sha256(archived.read_bytes()).hexdigest()
            if digest != manifest["files"][relative]:
                raise RuntimeError("Archived append source differs: " + relative)
            shutil.copyfile(archived, source / relative)
            restored[relative] = digest
        record["restoredProductionFiles"] = restored
        record["currentRegressionSha256"] = helpers.digest(source / "src-tauri/src/conversations/environment_history_tests/sqlite_usage_tests/acceptance/append.rs")
        target = work / "target"
        helpers.clone_debug_cache(arguments.cache.resolve(), target)
        binary, build = helpers.build(source, target, "before")
        record["build"] = build
        command = [str(binary), "--exact", TEST, "--test-threads=1"]
        environment = dict(os.environ, TUGBOAT_SQLITE_ACCEPTANCE_EXPORT=str(output / "payloads"))
        result = subprocess.run(command, cwd=ROOT, env=environment, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                timeout=120, check=False)
        (output / "run.log").write_text(result.stdout)
        expected = (result.returncode == 101 and "1 failed" in result.stdout
                    and "hydrate and one transaction-local prior read" in result.stdout
                    and re.search(r"left:\s+3\s+right:\s+2", result.stdout) is not None)
        record.update({"command": command, "exitCode": result.returncode,
                       "observedRawRecordReads": 3 if expected else None,
                       "requiredRawRecordReads": 2,
                       "status": "expected-failure-observed" if expected else "unexpected-result"})
    except Exception as failure:
        record["error"] = str(failure)
        raise
    finally:
        shutil.rmtree(work)
        record["generatedWorkRemoved"] = not work.exists()
        (output / "results.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record, indent=2))
    return int(record["status"] != "expected-failure-observed")


if __name__ == "__main__":
    raise SystemExit(main())
