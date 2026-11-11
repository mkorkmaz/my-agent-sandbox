# P02 reconciliation inputs

`history-primary.jsonl` and `history-child.jsonl` are filtered real Codex 0.167.0 guest records from a separate parent thread. The parent model/effort are GPT-6.1 Sol Max. The sole child was spawned with `fork_turns=all` and inherited those settings. No paid P01 sample was repeated. P01 files and hashes are unchanged.

`expected.json` lists every physical request's six token classes, source thread/turn/root-turn/response IDs, model, effort, recorded service tier and tier source. It contains own/descendant/combined/project totals, per-primary-prompt totals, disjoint response sets and the real inherited-history boundary. The previous CLI baseline is a separate scenario; its anonymized cwd does not place it in the new sample project.

The inherited child has two metadata headers and copied parent lifecycle records. The first header pins the owner. The copied parent header matches `forked_from_id`. An explicit owner `thread_settings_applied.thread_id` returns to the child's own stream before its own `task_started`. Timestamps and numeric ordinal values do not establish ownership. The copied completed and interrupted parent turns must not become child prompts or cancellations. No parent usage records were copied in this real paginated sample.

`synthetic-cases.json` is explicitly synthetic and is not provider evidence. It provides six small parser contracts: repeated/reordered notifications, reset counters with exact usage, missing provenance, malformed usage, a copied foreign request, and a zero request plus a genuinely empty own turn. These cases remain inputs for production parser tests in later plan steps.
