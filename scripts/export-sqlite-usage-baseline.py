#!/usr/bin/env python3
"""Export the legacy contract from a frozen, disposable pre-migration source copy."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--snapshot", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
arguments = parser.parse_args()
snapshot = arguments.snapshot.resolve(strict=True)
if not snapshot.is_relative_to(Path(tempfile.gettempdir()).resolve()):
    parser.error("The snapshot must be a disposable source copy in the temporary directory.")
root = Path(__file__).resolve().parent.parent
fixture = snapshot / "tests/fixtures/sqlite-usage/legacy-export.rs"
fixture.parent.mkdir(parents=True, exist_ok=True)
shutil.copyfile(root / "tests/fixtures/sqlite-usage/legacy-export.rs", fixture)
tests = snapshot / "src-tauri/src/conversations/environment_history_tests.rs"
include = '\ninclude!(concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/sqlite-usage/legacy-export.rs"));\n'
source = tests.read_text()
if include not in source:
    tests.write_text(source + include)
output = arguments.output.resolve()
output.mkdir(parents=True, exist_ok=True)
environment = dict(os.environ)
environment["CARGO_TARGET_DIR"] = str(snapshot / "target")
environment["TUGBOAT_SQLITE_BASELINE_EXPORT"] = str(output)
raise SystemExit(subprocess.run([
    "cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "--locked", "--offline",
    "sqlite_baseline_contract_exports", "--", "--nocapture",
], cwd=snapshot, env=environment).returncode)
