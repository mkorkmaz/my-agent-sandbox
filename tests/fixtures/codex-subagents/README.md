# Codex subagent record fixtures

`v0.167.0/` contains filtered records from real Linux guests started by Brig. No desktop rollout is used as guest proof. All observed lifecycle records are retained; none of these real turns lacks a token measurement. The new sample uses Codex App Server as a test driver. Its primary source is `vscode`; `guest-cli-baseline.jsonl` is a separate prior guest TUI sample with source `cli`.

The sample contains a primary with two user turns, two children, one grandchild, a follow-up turn on the same alpha child, and a separate normal fork. The previous CLI sample retains six completed and three interrupted turns. `lifecycle.json` retains filtered real archive, unarchive, normal-fork and ephemeral-fork observations. The ephemeral fork has no JSONL fixture because no durable rollout was created.

All IDs and working directories are replaced consistently. Dates are shifted by 313 days; order, elapsed times and numeric token values are preserved. Agent nicknames are replaced. The filter retains only version/source/lineage metadata, turn attribution, model/effort/tier, numeric usage, and lifecycle data. It removes prompts, answers, tool content, instructions, account IDs, credentials and rate limits. There are no synthetic scenarios in this set. `manifest.json` has the raw-source hash and fixture hash for each file.

These are JSONL parser inputs, not resumable Codex conversations. In particular, `session_id` can identify a shared root session; the file's `session_meta.id` and each request's `thread_id` identify the actual thread. A normal fork's `forked_from_id` does not make it a subagent. The observed paginated fork has inherited cumulative counters; its single `token_usage_record.usage` is the new request. Do not sum inherited counters as new spending.

The P01 evidence and verification script are under `artifacts/subagent-usage/`. Full usage reconciliation and inherited-history subagent coverage are separate P02 work. Native product acceptance is separate P14 work.
