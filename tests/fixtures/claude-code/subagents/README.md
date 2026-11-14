# Claude subagent fixture evidence

The JSONL and metadata files in this directory are synthetic test data. No Claude session or paid request produced these token counts.

The structure follows current primary sources checked on 2026-11-13:

- [Anthropic Agent SDK session reader](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/sessions.py) defines recursive `subagents/agent-<agentId>.jsonl` discovery below the root session UUID directory. It reads `toolUseId` and `parentAgentId` from adjacent `.meta.json` files.
- [Anthropic Agent SDK session tests](https://github.com/anthropics/claude-agent-sdk-python/blob/main/tests/test_sessions.py) cover root session IDs in child messages, nested workflow directories, and immediate parent IDs in sidecars.
- [Claude Code subagent documentation](https://code.claude.com/docs/en/sub-agents) describes separate transcripts, nested agents, and subagent resumption through the root session.

The existing `../v2.1.318/main.jsonl` contains genuine local acceptance records from Claude Code 2.1.318. This new fixture extends its message envelope with documented subagent markers and SDK metadata fields. The agent names, messages, tool IDs, UUIDs, counts, and relationship scenarios here are synthetic. This directory is format coverage, not a native subagent acceptance capture.

The test installs `root.jsonl` as `<rootUuid>.jsonl`. It installs alpha and beta directly below `<rootUuid>/subagents/`. It installs nested below `subagents/workflows/run-1/`. Alpha spawns nested. The root spawns alpha and beta. Input includes ordinary input, cache reads, and cache creation. Own totals are root 135, alpha 270, beta 540, and nested 405 tokens. Combined usage is 1,350 tokens across four physical messages. A synthetic tool-result summary advertises unrelated counts to prove that summaries are not measured again.

Failure tests extend these records with duplicates, missing files, invalid sidecars, cycles, conflicting IDs, and boundary cases. Those extensions are synthetic too.

`native-contract.json` contains the actual Rust dashboard and details output for this fixture. Guest namespace hashes and opaque revision values are replaced with stable fixture values. The frontend uses this contract to check root rows, nested details, and child-only refreshes. The `claude-fixture` model deliberately remains unpriced.
