#!/usr/bin/env python3
"""Run isolated Rust usage checks and record the acceptance matrix.

Compile the Rust test executable separately. This driver never starts the app,
calls an account service, invokes npm, or signs a file. All Rust fixtures used by
these filters create their own temporary records and guest directories.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = ROOT / "reports/sqlite-usage-evidence/p06/acceptance"
OUTPUT = DEFAULT_OUTPUT
PREFIX = "conversations::environment_history_tests::sqlite_usage_tests"

# Each row names an actual assertion, not only a test module or a package.
SCENARIOS = [
    ("A1", "typed-fields", "All typed fields, empty maps/turns, null and zero costs; cursor pending states and decoding limits", ["exact_schema_and_all_fields_round_trip_after_reopen", "typed_cursor_decoder_preserves_pending_states_and_bounds_invalid_input"]),
    ("A1", "v1-v2", "Raw v1/v2 prices, exact slots, keys and immutable backups", ["raw_v1_v2_import_preserves_slots_keys_prices_backups_and_reopen_identity"]),
    ("A1", "three-claude", "Three section 07 Claude contracts and Codex", ["sql_public_contracts_match_both_agents_and_three_claude_fixtures"]),
    ("A1", "pricing-evidence", "Persisted dated tariffs and ordered correction evidence", ["billing_metadata_tariffs_and_ordered_evidence_match_after_sql_reopen"]),
    ("A2", "scope-collisions", "Equal session IDs in two projects, agents and historical guests", ["equal_native_ids_stay_scoped_to_project_environment_and_guest", "codex_guest_switches_preserve_both_records_requests_cursors_and_states"]),
    ("A2", "lineage", "Orphan, unknown, fork, unlinked, malformed, exclusion and empty variant shapes", ["graph_orphan_unknown_fork_exclusion_and_empty_variant_shapes_match", "full_api_event_resume_and_quiet_contracts_match_on_both_agents_native_families"]),
    ("A2", "mixed-v1", "Mixed v1 guest row retains anonymous residual without duplicate charges", ["mixed_legacy_split_moves_the_anonymous_residual_without_replacing_its_row"]),
    ("A2", "registry-retention", "Actual environment removal and project forget retain saved SQL facts", ["registry_environment_removal_and_forget_leave_all_saved_sql_facts"]),
    ("A2", "family-status", "Archive, trash, restore preserve totals; live family blocks archive", ["full_api_event_resume_and_quiet_contracts_match_on_both_agents_native_families"]),
    ("A3", "file-replay", "Complete/partial lines, truncation, inode replacement and source loss", ["partial_codex_lines_rotation_truncation_and_source_loss_keep_recorded_facts"]),
    ("A3", "counter-exact", "Pending counter/exact pairing and duplicate physical request", ["pending_counter_exact_pairs_survive_import_restart_and_duplicate_occurrences"]),
    ("A3", "claude-owner", "Copied Claude response retains owner after child transcript disappears", ["claude_copied_child_evidence_keeps_the_saved_owner_when_its_transcript_is_missing"]),
    ("A3", "offline-first-read", "Dashboard/details-first pricing with absent guest, variants and rollback", ["sql_offline_claude_preparation_runs_before_dashboard_or_details_once_and_rolls_back_on_failure"]),
    ("A3", "billing-boundaries", "Malformed/missing/null metadata, cache classes and provider tariff boundaries", ["billing_metadata_tariffs_and_ordered_evidence_match_after_sql_reopen"]),
    ("A3", "dated-recovery", "Dated metadata recovery, conflicting verified evidence and cumulative output", ["live_claude_recovery_conflict_and_output_growth_keep_dated_evidence_after_replay"]),
    ("A3", "environment-preparation", "Saved pricing touches one environment and rejects a future policy", ["saved_pricing_sweeps_only_the_requested_environment_and_rejects_future_policy"]),
    ("A3", "append-header-rollback", "Single-record append preserves unrelated facts; source-loss metadata and cursor roll back and retry together", ["appending_one_record_preserves_unrelated_facts_and_atomically_saves_source_loss"]),
    ("A4", "transaction-death", "Real process death before/between request/cursor writes and before/after commit", ["killed_sql_writers_keep_request_cursor_and_rollup_in_one_commit"]),
    ("A4", "activation-death", "Real process death at eight stage, commit, parity, canonical and marker boundaries", ["interrupted_import_boundaries_recover_without_duplicates_or_json_writes"]),
    ("A4", "writer-busy", "External writer timeout preserves facts and cursor; retry is idempotent", ["a_busy_external_writer_times_out_without_advancing_saved_usage"]),
    ("A4", "disk-full", "Actual SQLite maximum-page limit rejects write atomically", ["sqlite_full_rolls_back_a_real_failed_write_and_keeps_the_store_reusable"]),
    ("A4", "sync-io", "Real database xSync failure restores committed state and reopens before public retry", ["failed_database_sync_restores_the_last_commit_before_reopen", "an_io_failure_requires_a_fresh_verified_connection_before_the_next_poll"]),
    ("A4", "corruption", "Interior page corruption and newer schema reject activation; later shared writes stop", ["interior_page_corruption_and_a_newer_schema_block_activation_without_json_fallback", "detected_corruption_blocks_later_writes_through_the_same_shared_handle"]),
    ("A4", "bad-input", "Invalid JSON, unknown fields, duplicate keys, integer overflow and mismatched v1 backup", ["malformed_unknown_duplicate_oversized_integer_and_backup_conflicts_never_activate"]),
    ("A4", "foreign-missing", "Foreign staging and missing active DB are preserved without recreation", ["foreign_staging_symlink_and_missing_active_database_are_preserved", "a_missing_or_linked_active_database_never_creates_a_replacement"]),
    ("A4", "old-binary", "Actual frozen v1 binary rejects activated dataset before JSON writes", ["the_actual_frozen_v1_binary_rejects_the_activated_dataset"]),
    ("A5", "empty-start", "Empty installation activates a private verified DB", ["empty_installation_activates_private_bound_database_without_receipts"]),
    ("A5", "move-return", "Closed database moves out and back with originals and permissions", ["a_closed_sqlite_dataset_moves_out_and_back_with_its_marker_and_originals"]),
    ("A5", "move-death", "Subprocess death before copy, after target marker and after pointer commit", ["interrupted_sqlite_moves_recover_before_copy_after_target_and_after_pointer_commit"]),
    ("A5", "hot-journal", "A genuine killed-writer rollback journal recovers before closed copy", ["hot_rollback_journals_recover_before_the_database_is_copied"]),
    ("A5", "locate-final-flush", "Located records receive final event before pointer commit; failed flush blocks restart", ["located_runtime_records_flush_the_final_native_request_before_pointer_commit"]),
    ("A5", "auth-final-flush", "Synthetic pending renewal rebinds only to same verified records dataset", ["a_pending_renewal_rebinds_only_to_the_verified_matching_records_dataset"]),
    ("A5", "wrong-target", "Partial/foreign targets, linked journals and replaced DBs are preserved", ["a_foreign_database_in_a_partial_target_is_never_overwritten", "linked_rollback_journals_block_a_move_without_changing_their_targets"]),
    ("A5", "second-instance", "Separate process cannot open or migrate leased records", ["a_second_instance_cannot_open_or_migrate_the_owned_records"]),
    ("A5", "close-retry", "Missing disk and actual SQLite busy-close preserve connection for retry", ["a_missing_records_folder_fails_close_without_recreating_files", "a_real_sqlite_close_error_retains_the_connection_and_can_be_retried"]),
    ("A6", "exact-api", "Complete DashboardUpdate, details and serialized event payloads match both stores", ["full_api_event_resume_and_quiet_contracts_match_on_both_agents_native_families"]),
    ("A6", "quiet-and-state", "Quiet poll reads zero raw records; normal status changes one row", ["unchanged_codex_scan_loads_no_raw_records_and_commits_no_data_change", "a_current_guest_state_change_updates_one_row_without_loading_raw_records"]),
    ("A6", "details-bound", "Details load only chosen historical slot without hidden/unrelated raw facts", ["historical_details_load_only_the_chosen_slot_without_hidden_or_unrelated_records"]),
    ("A6", "failed-publication", "Failed cursor or pricing transaction keeps caches and publishes no new usage", ["an_aborted_cursor_write_rolls_back_request_rows_and_does_not_publish", "failed_saved_claude_preparation_keeps_facts_and_both_history_caches_unchanged"]),
]


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def parse_tests(output):
    # A killed child process can write directly to its parent's stdout between
    # the test name and final status, even when libtest capture is enabled.
    starts = list(re.finditer(r"^test ([A-Za-z0-9_:]+) \.\.\. ", output, re.MULTILINE))
    tests = {}
    for index, match in enumerate(starts):
        end = starts[index + 1].start() if index + 1 < len(starts) else len(output)
        block = output[match.end():end]
        status = re.search(r"^(ok|FAILED|ignored)(?:,.*)?$", block, re.MULTILINE)
        if status:
            tests[match.group(1)] = status.group(1)
    return tests


def run(binary, name, arguments, environment):
    start = time.monotonic()
    command = [str(binary), *arguments, "--test-threads=1"]
    result = subprocess.run(command, cwd=ROOT, env=environment, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            timeout=900, check=False)
    log = OUTPUT / f"{name}.log"
    log.write_text(result.stdout)
    tests = parse_tests(result.stdout)
    record = {"command": command, "exitCode": result.returncode,
              "elapsedSeconds": time.monotonic() - start, "tests": tests,
              "log": str(log.relative_to(ROOT))}
    write(OUTPUT / f"{name}.json", record)
    return record


def main():
    global OUTPUT
    import os
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--old-binary", type=Path)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    arguments = parser.parse_args()
    OUTPUT = arguments.output.resolve()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ, TUGBOAT_SQLITE_ACCEPTANCE_EXPORT=str(OUTPUT / "payloads"))
    groups = [("public", [PREFIX]), ("store", ["usage_store::"]),
              ("location", ["data_location::"]), ("lifecycle", ["lifecycle::"]),
              ("auth-reconnect", ["shared_chatgpt::tests::a_pending_renewal_rebinds_only_to_the_verified_matching_records_dataset"])]
    results = [run(arguments.binary, name, filters, environment) for name, filters in groups]
    if arguments.old_binary:
        environment["TUGBOAT_SQLITE_OLD_BINARY"] = str(arguments.old_binary.resolve())
        results.append(run(arguments.binary, "old-v1", ["--ignored", "--exact", PREFIX + "::acceptance::the_actual_frozen_v1_binary_rejects_the_activated_dataset"], environment))
    tests = {name: status for result in results for name, status in result["tests"].items() if status != "ignored"}
    rows = []
    for gate, name, description, required in SCENARIOS:
        matches = {suffix: next((status for test, status in tests.items() if test.endswith("::" + suffix)), "notRun") for suffix in required}
        status = "passed" if all(value == "ok" for value in matches.values()) else "notRun"
        if "FAILED" in matches.values():
            status = "failed"
        evidence = [result["log"] for result in results if any(
            test.endswith("::" + suffix) for test in result["tests"] for suffix in required)]
        rows.append({"gate": gate, "scenario": name, "description": description,
                     "status": status, "tests": matches, "evidence": evidence})
    volume_path = DEFAULT_OUTPUT / "volume-driver/results.json"
    volume = json.loads(volume_path.read_text()) if volume_path.exists() else None
    rows.append({"gate": "A5", "scenario": "real-disposable-volume", "status": "passed" if volume and volume.get("status") == "passed" else "notRun", "evidence": str(volume_path.relative_to(ROOT)), "description": "Temporary APFS image detach, unavailable probe, reattach and move back; no physical hardware unplug claimed"})
    limits = [{"scenario": "native-ui-event-delivery", "status": "notRun", "reason": "No app launch is authorized; exact backend event serialization is tested."}, {"scenario": "physical-hardware-unplug", "status": "notRun", "reason": "Actual disposable APFS image detach was used; no user disk was touched."}]
    sources = sorted((ROOT / "src-tauri/src/usage_store").rglob("*.rs")) + sorted((ROOT / "src-tauri/src/conversations/environment_history_tests/sqlite_usage_tests").rglob("*.rs"))
    sources += [ROOT / path for path in ["src-tauri/src/usage_store/migrations/001_initial.sql", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/src/conversations/mod.rs", "src-tauri/src/conversations/sqlite_benchmark.rs", "src-tauri/src/conversations/sql_backend.rs", "src-tauri/src/conversations/sql_projection.rs", "src-tauri/src/conversations/sql_scan.rs", "src-tauri/src/conversations/claude/history.rs", "src-tauri/src/storage/data_location/mod.rs", "src-tauri/src/desktop/lifecycle/mod.rs", "src-tauri/src/main.rs"]]
    hashes = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sources}
    summary = {"schemaVersion": 1, "generatedFixturesOnly": True, "binary": str(arguments.binary.resolve()), "binarySha256": hashlib.sha256(arguments.binary.read_bytes()).hexdigest(), "runs": results, "scenarios": rows, "limits": limits, "sourceSha256": hashes}
    write(OUTPUT / "matrix.json", summary)
    print(json.dumps({"matrix": str(OUTPUT / "matrix.json"), "failedRuns": sum(result["exitCode"] != 0 for result in results), "scenarios": {status: sum(row["status"] == status for row in rows) for status in ["passed", "failed", "notRun"]}}, indent=2))
    return int(any(result["exitCode"] != 0 for result in results) or any(row["status"] != "passed" for row in rows))


if __name__ == "__main__":
    raise SystemExit(main())
