-- Preserve usage keys referenced by records outside the retired project scopes.
CREATE TABLE usage_environments_shared (
    environment_key INTEGER PRIMARY KEY,
    project_id TEXT NOT NULL CHECK (length(project_id) = 32),
    environment_id TEXT NOT NULL CHECK (length(environment_id) = 32),
    agent_kind TEXT NOT NULL CHECK (agent_kind IN ('codex', 'claude-code')),
    claude_pricing_version INTEGER NOT NULL DEFAULT 0
        CHECK (claude_pricing_version >= 0 AND (agent_kind = 'claude-code' OR claude_pricing_version = 0)),
    warnings_json TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(warnings_json) AND json_type(warnings_json) = 'array'),
    UNIQUE (project_id, environment_id, agent_kind)
) STRICT;
INSERT INTO usage_environments_shared SELECT * FROM usage_environments;
DROP TABLE usage_environments;
ALTER TABLE usage_environments_shared RENAME TO usage_environments;
PRAGMA user_version = 2;
