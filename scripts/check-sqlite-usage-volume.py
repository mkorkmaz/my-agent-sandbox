#!/usr/bin/env python3
"""Verify records moves with a disposable APFS image. Never launch Tugboat."""
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time
import uuid


repository = Path(__file__).resolve().parent.parent
output = repository / "reports/sqlite-usage-evidence/p06/acceptance/volume-driver"
output.mkdir(parents=True, exist_ok=True)
fixture = Path(tempfile.mkdtemp(prefix="tugboat-p06-volume-")).resolve()
mount = fixture / "volume"
mount.mkdir(mode=0o700)
image = fixture / "fixture.sparseimage"
label = "TugboatP06" + uuid.uuid4().hex[:10]
owner = {"kind": "tugboat-p06-volume-fixture-v1", "mount": str(mount)}
(fixture / "fixture-owner.json").write_text(json.dumps(owner) + "\n")
report = {
    "kind": "disposable-APFS-volume-acceptance",
    "fixtureRoot": str(fixture),
    "mount": str(mount),
    "image": str(image),
    "volumeLabel": label,
    "status": "running",
    "commands": [],
    "phases": [],
}
mounted_device = None


def save():
    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")


def run(command, name, environment=None):
    started = time.monotonic()
    result = subprocess.run(
        command, cwd=repository, env=environment, capture_output=True, timeout=240
    )
    (output / f"{name}.stdout").write_bytes(result.stdout)
    (output / f"{name}.stderr").write_bytes(result.stderr)
    row = {
        "phase": name,
        "command": command,
        "exitCode": result.returncode,
        "seconds": time.monotonic() - started,
    }
    report["commands"].append(row)
    save()
    print(json.dumps(row), flush=True)
    if result.returncode:
        raise RuntimeError(f"{name} failed; preserve its output in {output}")
    return result.stdout


def attach(name):
    global mounted_device
    payload = run(
        ["hdiutil", "attach", "-nobrowse", "-noautoopen", "-mountpoint", str(mount),
         "-owners", "on", "-plist", str(image)], name
    )
    entities = plistlib.loads(payload)["system-entities"]
    assert any(Path(item.get("mount-point", "/")).resolve() == mount for item in entities)
    assert os.path.ismount(mount)
    mounted_device = mount.stat().st_dev


def detach(name):
    global mounted_device
    assert mounted_device is not None and os.path.ismount(mount)
    assert mount.parent == fixture and mount.stat().st_dev == mounted_device
    assert json.loads((fixture / "fixture-owner.json").read_text()) == owner
    run(["hdiutil", "detach", str(mount)], name)
    assert not os.path.ismount(mount)
    mounted_device = None


def phase(name):
    environment = dict(os.environ)
    environment["TUGBOAT_P06_VOLUME_ROOT"] = str(fixture)
    environment["TUGBOAT_P06_VOLUME_PHASE"] = name
    run(
        ["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "--locked", "--offline",
         "storage::data_location::p06_volume_tests::p06_disposable_volume_worker", "--", "--exact", "--ignored", "--nocapture"],
        name, environment,
    )
    result = json.loads((fixture / f"{name}.json").read_text())
    report["phases"].append(result)
    save()


try:
    run(["hdiutil", "create", "-size", "256m", "-fs", "APFS", "-volname", label,
         "-type", "SPARSE", "-nospotlight", "-o", str(image), "-plist"], "create")
    attach("attach")
    phase("seed-move")
    detach("detach")
    phase("detached-probe")
    attach("reattach")
    phase("move-back")
    report["status"] = "passed"
except Exception as error:
    report["status"] = "failed"
    report["error"] = str(error)
    raise
finally:
    if mounted_device is not None:
        try:
            detach("cleanup-detach")
        except Exception as error:
            report["cleanupError"] = str(error)
            report["status"] = "failed"
    if mounted_device is None:
        shutil.rmtree(fixture)
        report["fixtureRemoved"] = True
    save()
