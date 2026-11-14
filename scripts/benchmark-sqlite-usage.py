#!/usr/bin/env python3
"""Build and compare real Rust usage paths on generated, same-volume fixtures."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
ORIGINAL_EVIDENCE = ROOT / "reports/sqlite-usage-evidence/p06/performance"
EVIDENCE = ORIGINAL_EVIDENCE
OPERATIONS = ["status", "details", "dashboard", "dashboard_live", "quiet", "append"]
WORK = ROOT / ".build/p06-benchmark"
PREFIX = "TUGBOAT_BENCH "
TEST = "conversations::sqlite_benchmark::worker"


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def add_hook(path, name, phase):
    source = path.read_text()
    marker = f"fn {name}("
    if marker not in source:
        return
    start = source.index(marker)
    body = source.index("{", start) + 1
    source = source[:body] + f"\n    let _benchmark_phase = crate::conversations::sqlite_benchmark::Phase::{phase}();" + source[body:]
    path.write_text(source)


def baseline_source():
    manifest = json.loads((ROOT / "reports/sqlite-usage-evidence/p01/source-manifest.json").read_text())
    original = Path(manifest["snapshot"])
    destination = WORK / "json-source"
    if not destination.exists():
        shutil.copytree(original, destination, symlinks=True, ignore=shutil.ignore_patterns("target", "node_modules", ".build"))
        (destination / ".build").symlink_to(original / ".build", target_is_directory=True)
        production = {}
        for relative in ["src-tauri/src/conversations.rs", "src-tauri/src/conversations/claude_history.rs", "src-tauri/src/usage/cursor.rs", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock"]:
            actual = hashlib.sha256((destination / relative).read_bytes()).hexdigest()
            if actual != manifest["files"][relative]:
                raise RuntimeError(f"Frozen P01 production source differs: {relative}")
            production[relative] = actual
        source = destination / "src-tauri/src/conversations.rs"
        source.write_text(source.read_text() + "\n#[cfg(test)]\npub(crate) mod sqlite_benchmark;\n")
        add_hook(source, "load_ledger", "backend")
        add_hook(source, "write_ledger", "backend")
        add_hook(source, "scan_history_known", "parser")
        add_hook(destination / "src-tauri/src/conversations/claude_history.rs", "scan_known", "parser")
        add_hook(destination / "src-tauri/src/usage/cursor.rs", "sync_file_bound", "parser")
        write_json(EVIDENCE / "baseline-source.json", {"commit": manifest["commit"], "originalSnapshot": str(original), "benchmarkCopy": str(destination), "verifiedBeforeTestHooks": production, "productionChanges": "None. Test module include and inactive timing scopes only."})
    module = (ROOT / "src-tauri/src/conversations/sqlite_benchmark.rs").read_text().replace("tugboat_json_baseline", "all()")
    (destination / "src-tauri/src/conversations/sqlite_benchmark.rs").write_text(module)
    return destination


def build(source, backend):
    log_path = EVIDENCE / f"build-{backend}.log"
    command = ["cargo", "test", "--manifest-path", str(source / "src-tauri/Cargo.toml"), "--release", "--locked", "--offline", "--no-run", "--message-format=json", TEST]
    environment = dict(os.environ)
    if backend == "json":
        environment["CARGO_TARGET_DIR"] = str(source / "target")
    process = subprocess.Popen(command, cwd=source, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    executable = None
    profile = None
    with log_path.open("w") as log:
        for line in process.stdout:
            log.write(line)
            try:
                item = json.loads(line)
            except json.JSONDecodeError:
                continue
            if item.get("reason") == "compiler-artifact" and item.get("executable") and item.get("profile", {}).get("test"):
                executable = item["executable"]
                profile = item["profile"]
    if process.wait() != 0 or not executable:
        raise RuntimeError(f"Rust {backend} release test build failed; see {log_path}")
    digest = hashlib.sha256(Path(executable).read_bytes()).hexdigest()
    preserved = EVIDENCE / "binaries" / f"tugboat-{backend}-{digest[:16]}"
    preserved.parent.mkdir(parents=True, exist_ok=True)
    if preserved.exists() and hashlib.sha256(preserved.read_bytes()).hexdigest() != digest:
        raise RuntimeError("A preserved benchmark binary has conflicting contents")
    if not preserved.exists():
        shutil.copy2(executable, preserved)
    return {"executable": str(preserved), "executedPath": executable, "preservedPath": str(preserved), "command": command, "profile": profile, "sha256": digest}


class Worker:
    def __init__(self, backend, binary, root, sessions, generate=False, log_tag=""):
        self.log = (EVIDENCE / f"worker-{sessions}-{backend}{'-generate' if generate else ''}{log_tag}.log").open("w")
        environment = dict(os.environ, TUGBOAT_BENCH_ROOT=str(root), TUGBOAT_BENCH_SESSIONS=str(sessions))
        if generate:
            environment["TUGBOAT_BENCH_GENERATE"] = "1"
        self.process = subprocess.Popen([binary, "--ignored", "--exact", TEST, "--nocapture", "--test-threads=1"], cwd=ROOT, env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
        self.initial = self.read()

    def read(self):
        for line in self.process.stdout:
            self.log.write(line)
            self.log.flush()
            if PREFIX in line:
                return json.loads(line.split(PREFIX, 1)[1])
        raise RuntimeError(f"Benchmark worker failed: {self.process.poll()}")

    def call(self, op, step=0):
        self.process.stdin.write(json.dumps({"op": op, "step": step}) + "\n")
        self.process.stdin.flush()
        return self.read()

    def stop(self):
        if self.process.poll() is None:
            self.process.stdin.write('{"op":"stop"}\n')
            self.process.stdin.close()
            self.process.stdout.read()
            if self.process.wait(timeout=30) != 0:
                raise RuntimeError("Benchmark worker failed at shutdown")
        self.log.close()


def describe(values):
    values = sorted(values)
    return {"samples": len(values), "medianMs": statistics.median(values) / 1e6, "p95Ms": values[math.ceil(.95 * len(values)) - 1] / 1e6, "minMs": values[0] / 1e6, "maxMs": values[-1] / 1e6}


def compare_results(left, right):
    left, right = left["result"], right["result"]
    if left.keys() != right.keys():
        raise RuntimeError(f"Result shape mismatch: {left} != {right}")
    for key in left:
        if key == "costUsd":
            if abs(left[key] - right[key]) > 1e-12:
                raise RuntimeError("Public benchmark costs differ")
            continue
        if left[key] != right[key]:
            raise RuntimeError(f"Benchmark {key} mismatch: {left} != {right}")


def run(builds, repetitions, warmups, sizes, fixture_parent, runtime_only=False, operations=OPERATIONS):
    output = {}
    for sessions in sizes:
        root = Path(tempfile.mkdtemp(prefix=f"tugboat-usage-benchmark-{sessions}-", dir=fixture_parent))
        (root / ".generated-tugboat-benchmark").write_text("Generated fixture. No user data.\n")
        generator = Worker("json", builds["json"]["executable"], root, sessions, True)
        generated = generator.initial
        generator.process.wait(timeout=120)
        generator.log.close()
        workers = {backend: Worker(backend, builds[backend]["executable"], root, sessions) for backend in ["json", "sql"]}
        if any(worker.initial["device"] != root.stat().st_dev for worker in workers.values()):
            raise RuntimeError("Both backend fixtures must use the same local volume")
        metadata = {"generatedRoot": str(root), "volumeDevice": root.stat().st_dev, "rootInode": root.stat().st_ino, "scopePerProject": {"sessions": sessions, "requests": sessions * 8, "environments": 2}, "projects": 2, "generated": generated, "storage": {backend: worker.initial for backend, worker in workers.items()}}
        write_json(EVIDENCE / f"fixture-{sessions}.json", metadata)
        all_samples = []
        metrics = {}
        for op in operations:
            samples = {"json": [], "sql": []}
            for step in range(warmups + repetitions):
                if op == "append":
                    workers["json"].call("append-source", step)
                order = ["json", "sql"] if step % 2 == 0 else ["sql", "json"]
                pair = {}
                for backend in order:
                    result = workers[backend].call(op, step)
                    pair[backend] = result
                    if backend == "sql" and op == "status" and result["dataChanges"] != 1:
                        raise RuntimeError(f"Status changed {result['dataChanges']} rows, expected one")
                    if backend == "sql" and op == "quiet" and result["dataChanges"] != 0:
                        raise RuntimeError("Quiet polling made persistent data changes")
                    if step >= warmups:
                        sample = dict(result, backend=backend, sample=step - warmups, order=order.index(backend), sessions=sessions)
                        samples[backend].append(sample)
                        all_samples.append(sample)
                compare_results(pair["json"], pair["sql"])
                if step == warmups or (step >= warmups and (step - warmups + 1) % 25 == 0):
                    print(f"{sessions} {op}: {max(0, step - warmups + 1)}/{repetitions}", flush=True)
            metrics[op] = {backend: {field: describe([sample[field] for sample in rows]) for field in ["endToEndNs", "guestParseNs", "backendNs", "otherNs"]} for backend, rows in samples.items()}
            write_json(EVIDENCE / f"raw-{sessions}.json", all_samples)
        if not runtime_only:
            migration = []
            for step in range(warmups + repetitions):
                result = workers["sql"].call("migration", step)
                if step >= warmups:
                    migration.append(dict(result, sample=step - warmups))
                if step >= warmups and (step - warmups + 1) % 25 == 0:
                    print(f"{sessions} migration: {step - warmups + 1}/{repetitions}", flush=True)
            write_json(EVIDENCE / f"raw-migration-{sessions}.json", migration)
            metrics["migration"] = {field: describe([sample[field] for sample in migration]) for field in ["importNs", "prepareNs", "openNs"]}
        if runtime_only:
            initial = json.loads((ORIGINAL_EVIDENCE / "initial-run/measurements.json").read_text())
            metrics["migration"] = initial[str(sessions)]["metrics"]["migration"]
        write_json(EVIDENCE / f"diagnostics-{sessions}.json", workers["sql"].call("diagnostics"))
        for worker in workers.values():
            worker.stop()
        output[str(sessions)] = {"fixture": metadata, "metrics": metrics}
        write_json(EVIDENCE / "measurements.json", output)
    return output


def run_steady(builds, repetitions, warmups, sizes, fixture_parent):
    results = {}
    for sessions in sizes:
        root = Path(tempfile.mkdtemp(prefix=f"tugboat-usage-steady-{sessions}-", dir=fixture_parent))
        (root / ".generated-tugboat-benchmark").write_text("Generated fixture. No user data.\n")
        generator = Worker("json", builds["json"]["executable"], root, sessions, True, "-steady")
        generator.process.wait(timeout=120)
        generator.log.close()
        worker = Worker("sql", builds["sql"]["executable"], root, sessions, log_tag="-steady")
        preparation_samples = []
        for step in range(warmups + repetitions):
            value = worker.call("prepare_copy", step)
            if step >= warmups:
                preparation_samples.append(dict(value, sample=step - warmups))
        write_json(EVIDENCE / f"raw-final-first-prep-{sessions}.json", preparation_samples)
        samples = []
        for step in range(warmups + repetitions):
            value = worker.call("steady_open", step)
            if step >= warmups:
                samples.append(dict(value, sample=step - warmups))
        worker.stop()
        write_json(EVIDENCE / f"raw-steady-open-{sessions}.json", samples)
        results[str(sessions)] = {"generatedRoot": str(root), "volumeDevice": root.stat().st_dev, "metric": describe([sample["steadyOpenNs"] for sample in samples]), "firstPreparation": describe([sample["firstPreparationNs"] for sample in preparation_samples]), "preparationScopeAndReopenGuardPassed": all(sample["policyVersions"] == [["11111111111111111111111111111111", 1], ["22222222222222222222222222222222", 0]] and sample["reopenSkipped"] for sample in preparation_samples), "allPolicySweepsSkipped": all(sample["policySweepSkipped"] for sample in samples), "allRawRecordLoadsZero": all(sample["rawRecordsLoaded"] == 0 for sample in samples), "allDataChangesZero": all(sample["dataChanges"] == 0 for sample in samples), "build": builds["sql"]}
    write_json(EVIDENCE / "steady-open.json", results)


def main():
    global EVIDENCE
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", "--output", dest="evidence_dir", type=Path, default=ORIGINAL_EVIDENCE)
    parser.add_argument("--operations", nargs="+", choices=OPERATIONS, default=OPERATIONS)
    parser.add_argument("--build-only", action="store_true")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--steady-only", action="store_true")
    parser.add_argument("--runtime-only", action="store_true")
    parser.add_argument("--reuse-json", action="store_true")
    parser.add_argument("--repetitions", type=int, default=100)
    parser.add_argument("--warmups", type=int, default=5)
    parser.add_argument("--sizes", type=int, nargs="+", default=[10, 1000])
    parser.add_argument("--fixture-parent", type=Path, default=WORK)
    args = parser.parse_args()
    if args.repetitions < 1 or args.warmups < 0:
        parser.error("Use at least one repetition and a nonnegative warmup count.")
    if any(size < 6 or size % 2 for size in args.sizes):
        parser.error("Each fixture size must be even and include the three-session family.")
    if len(args.operations) != len(set(args.operations)):
        parser.error("Each benchmark operation may appear only once.")
    EVIDENCE = args.evidence_dir.resolve()
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    WORK.mkdir(parents=True, exist_ok=True)
    if args.skip_build:
        builds = json.loads((EVIDENCE / "builds.json").read_text())
    else:
        if args.reuse_json:
            baseline = json.loads((ORIGINAL_EVIDENCE / "initial-run/builds.json").read_text())["json"]
            baseline["executable"] = baseline["preservedPath"]
            if hashlib.sha256(Path(baseline["executable"]).read_bytes()).hexdigest() != baseline["sha256"]:
                raise RuntimeError("The preserved P01 release binary changed")
            builds = {"json": baseline, "sql": build(ROOT, "sql")}
        if not args.reuse_json:
            builds = {"json": build(baseline_source(), "json"), "sql": build(ROOT, "sql")}
        write_json(EVIDENCE / "builds.json", builds)
    for backend, metadata in builds.items():
        if hashlib.sha256(Path(metadata["executable"]).read_bytes()).hexdigest() != metadata["sha256"]:
            raise RuntimeError(f"The preserved {backend} benchmark binary changed")
    if args.build_only:
        return
    if args.steady_only:
        run_steady(builds, args.repetitions, args.warmups, args.sizes, args.fixture_parent.resolve(strict=True))
        return
    start = time.time()
    results = run(builds, args.repetitions, args.warmups, args.sizes, args.fixture_parent.resolve(strict=True), args.runtime_only, args.operations)
    gates = {}
    if "10" in results and "status" in results["10"]["metrics"]:
        metric = results["10"]["metrics"]["status"]
        extra = metric["sql"]["endToEndNs"]["medianMs"] - metric["json"]["endToEndNs"]["medianMs"]
        gates["smallStatusAdditionalMedianAtMost10ms"] = {"valueMs": extra, "passed": extra <= 10}
    if "1000" in results:
        metrics = results["1000"]["metrics"]
        for op, field, limit in [("status", "medianMs", .8), ("details", "p95Ms", 1.1), ("dashboard", "p95Ms", 1.1), ("dashboard_live", "p95Ms", 1.1)]:
            if op not in metrics:
                continue
            ratio = metrics[op]["sql"]["endToEndNs"][field] / metrics[op]["json"]["endToEndNs"][field]
            gates[f"large_{op}_{field}"] = {"ratio": ratio, "limit": limit, "passed": ratio <= limit}
    for size in ["10", "1000"]:
        if size not in results or "append" not in results[size]["metrics"]:
            continue
        metric = results[size]["metrics"]["append"]
        for field in ["medianMs", "p95Ms"]:
            ratio = metric["sql"]["endToEndNs"][field] / metric["json"]["endToEndNs"][field]
            gates[f"sessions{size}_append_{field}"] = {"ratio": ratio, "limit": 1.0, "passed": ratio <= 1.0}
    complete_samples = args.repetitions == 100 and args.warmups == 5
    complete_matrix = complete_samples and args.sizes == [10, 1000] and args.operations == OPERATIONS
    original_gates = [gate for name, gate in gates.items() if "_append_" not in name]
    append_gates = [gate for name, gate in gates.items() if "_append_" in name]
    original_passed = complete_matrix and len(original_gates) == 5 and all(gate["passed"] for gate in original_gates)
    append_passed = complete_matrix and len(append_gates) == 4 and all(gate["passed"] for gate in append_gates)
    passed = original_passed and append_passed
    write_json(EVIDENCE / "gates.json", {"repetitions": args.repetitions, "warmups": args.warmups, "operations": args.operations, "sizes": args.sizes, "completeSampleCount": complete_samples, "completeMatrix": complete_matrix, "originalFiveGatesPassed": original_passed, "appendFourGatesPassed": append_passed, "allRequiredGatesPassed": passed, "seconds": time.time() - start, "gates": gates})
    print(json.dumps(gates, indent=2), flush=True)
    if complete_matrix and not passed:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
