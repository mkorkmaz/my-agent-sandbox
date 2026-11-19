-- Create only in a new staging database. Configure the connection first.
-- The importer owns BEGIN IMMEDIATE / COMMIT around DDL + store_identity.
-- Import legacy records and receipts in the next transaction on this staging file.
PRAGMA application_id = 0x54554755; -- TUGU
PRAGMA user_version = 1;

CREATE TABLE store_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    records_directory_id TEXT NOT NULL CHECK (length(records_directory_id) = 32)
) STRICT;

CREATE TABLE usage_environments (
    environment_key INTEGER PRIMARY KEY,
    project_id TEXT NOT NULL CHECK (length(project_id) = 32),
    environment_id TEXT NOT NULL CHECK (length(environment_id) = 32),
    agent_kind TEXT NOT NULL CHECK (agent_kind IN ('codex', 'claude-code')),
    -- 0 = not prepared; 1 = current saved-Claude pricing policy. Codex stays at 0.
    claude_pricing_version INTEGER NOT NULL DEFAULT 0
        CHECK (claude_pricing_version >= 0 AND (agent_kind = 'claude-code' OR claude_pricing_version = 0)),
    warnings_json TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(warnings_json) AND json_type(warnings_json) = 'array'),
    UNIQUE (project_id, environment_id)
) STRICT;

CREATE TABLE session_groups (
    session_key INTEGER PRIMARY KEY,
    environment_key INTEGER NOT NULL REFERENCES usage_environments,
    native_session_id TEXT NOT NULL CHECK (length(native_session_id) = 36),
    has_variants_bucket INTEGER NOT NULL CHECK (has_variants_bucket IN (0, 1)),
    UNIQUE (environment_key, native_session_id)
) STRICT;

CREATE TABLE session_records (
    record_id INTEGER PRIMARY KEY,
    session_key INTEGER NOT NULL REFERENCES session_groups,
    variant_key TEXT, -- NULL: sessions[id]; otherwise variants[id][key], including ''.
    guest_namespace TEXT, -- The field on the record, independent of its map key.
    agent_kind TEXT CHECK (agent_kind IN ('codex', 'claude-code')),
    title TEXT NOT NULL,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    state TEXT NOT NULL CHECK (state IN ('active', 'archived', 'deleted')),
    available INTEGER NOT NULL CHECK (available IN (0, 1)),
    parser_version INTEGER NOT NULL CHECK (parser_version BETWEEN 0 AND 4294967295),
    warning TEXT,
    lineage_json TEXT NOT NULL CHECK (json_valid(lineage_json) AND json_type(lineage_json) = 'object'),
    lineage_problem TEXT CHECK (lineage_problem IN
        ('malformed','unsupportedVersion','unsupportedSource','conflicting','missingSource')),
    source_root_id TEXT,
    exclusion_root_id TEXT,
    exclusion_root_cwd TEXT,
    claude_source_json TEXT CHECK (claude_source_json IS NULL OR
        (json_valid(claude_source_json) AND json_type(claude_source_json) = 'object')),
    CHECK ((exclusion_root_id IS NULL) = (exclusion_root_cwd IS NULL)),
    UNIQUE (session_key, variant_key)
) STRICT;

-- UNIQUE above covers non-NULL slots. SQLite permits duplicate NULLs in UNIQUE.
CREATE UNIQUE INDEX uq_selected_record ON session_records(session_key)
    WHERE variant_key IS NULL;

CREATE TABLE usage_turns (
    turn_key INTEGER PRIMARY KEY,
    record_id INTEGER NOT NULL REFERENCES session_records ON DELETE CASCADE,
    bucket_key TEXT NOT NULL, -- Legacy map key; do not replace it with turn.id.
    native_turn_id TEXT NOT NULL,
    status TEXT NOT NULL,
    origin_thread_id TEXT,
    root_turn_id TEXT,
    parent_turn_id TEXT,
    UNIQUE (record_id, bucket_key)
) STRICT;

CREATE TABLE usage_requests (
    turn_key INTEGER NOT NULL REFERENCES usage_turns ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    entry_key TEXT NOT NULL,
    recorded_at TEXT NOT NULL, -- Retain the source string, including its precision.
    model TEXT,
    input_tokens INTEGER NOT NULL CHECK (input_tokens >= 0),
    cached_tokens INTEGER NOT NULL CHECK (cached_tokens >= 0),
    cache_write_tokens INTEGER NOT NULL CHECK (cache_write_tokens >= 0),
    output_tokens INTEGER NOT NULL CHECK (output_tokens >= 0),
    reasoning_tokens INTEGER NOT NULL CHECK (reasoning_tokens BETWEEN 0 AND output_tokens),
    write_12h_tokens INTEGER CHECK (write_12h_tokens BETWEEN 0 AND cache_write_tokens),
    cache_known INTEGER NOT NULL CHECK (cache_known IN (0, 1)),
    cached_known INTEGER CHECK (cached_known IN (0, 1)),
    cache_write_known INTEGER CHECK (cache_write_known IN (0, 1)),
    partial INTEGER NOT NULL CHECK (partial IN (0, 1)),
    cost_usd REAL CHECK (cost_usd >= 0),
    unpriced_reason TEXT,
    service_tier TEXT,
    tier_source TEXT,
    rates_json TEXT CHECK (rates_json IS NULL OR (json_valid(rates_json) AND json_type(rates_json) = 'object')),
    rate_correction_json TEXT CHECK (rate_correction_json IS NULL OR (json_valid(rate_correction_json) AND json_type(rate_correction_json) = 'object')),
    provider_correction_json TEXT CHECK (provider_correction_json IS NULL OR (json_valid(provider_correction_json) AND json_type(provider_correction_json) = 'object')),
    amount_correction_json TEXT CHECK (amount_correction_json IS NULL OR (json_valid(amount_correction_json) AND json_type(amount_correction_json) = 'object')),
    anthropic_json TEXT CHECK (anthropic_json IS NULL OR (json_valid(anthropic_json) AND json_type(anthropic_json) = 'object')),
    anthropic_corrections_json TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(anthropic_corrections_json) AND json_type(anthropic_corrections_json) = 'array'),
    source_guest TEXT,
    model_provider TEXT,
    origin_thread_id TEXT,
    origin_turn_id TEXT,
    root_turn_id TEXT,
    response_id TEXT,
    CHECK (cached_tokens <= input_tokens - cache_write_tokens),
    CHECK ((source_guest IS NULL AND model_provider IS NULL AND origin_thread_id IS NULL
            AND response_id IS NULL AND origin_turn_id IS NULL AND root_turn_id IS NULL)
        OR (source_guest IS NOT NULL AND model_provider IS NOT NULL
            AND origin_thread_id IS NOT NULL AND response_id IS NOT NULL)),
    PRIMARY KEY (turn_key, ordinal)
) STRICT, WITHOUT ROWID;

CREATE INDEX ix_request_source ON usage_requests
    (source_guest, model_provider, response_id, origin_thread_id)
    WHERE response_id IS NOT NULL;

CREATE TABLE source_cursors (
    record_id INTEGER NOT NULL REFERENCES session_records ON DELETE CASCADE,
    source_path BLOB NOT NULL CHECK (length(source_path) > 0),
    cursor_json TEXT NOT NULL CHECK (json_valid(cursor_json) AND json_type(cursor_json) = 'object'),
    PRIMARY KEY (record_id, source_path)
) STRICT, WITHOUT ROWID;

CREATE TABLE session_rollups (
    record_id INTEGER PRIMARY KEY REFERENCES session_records ON DELETE CASCADE,
    own_summary_json TEXT NOT NULL CHECK (json_valid(own_summary_json) AND json_type(own_summary_json) = 'object'),
    resolved_guest_namespace TEXT, -- Cached record_namespace(id, record), possibly unknown.
    has_stored_turns INTEGER NOT NULL CHECK (has_stored_turns IN (0, 1)),
    sourced_request_count INTEGER NOT NULL CHECK (sourced_request_count >= 0),
    missing_turn_links INTEGER NOT NULL CHECK (missing_turn_links IN (0, 1)),
    has_unread_bytes INTEGER NOT NULL CHECK (has_unread_bytes IN (0, 1)),
    has_ownership_problem INTEGER NOT NULL CHECK (has_ownership_problem IN (0, 1)),
    usage_revision TEXT NOT NULL CHECK (length(usage_revision) = 64)
) STRICT;

CREATE TABLE legacy_imports (
    relative_path TEXT PRIMARY KEY,
    environment_key INTEGER NOT NULL UNIQUE REFERENCES usage_environments,
    ledger_version INTEGER NOT NULL CHECK (ledger_version IN (1, 2)),
    source_sha256 BLOB NOT NULL CHECK (length(source_sha256) = 32),
    source_bytes INTEGER NOT NULL CHECK (source_bytes >= 0),
    record_count INTEGER NOT NULL CHECK (record_count >= 0),
    turn_count INTEGER NOT NULL CHECK (turn_count >= 0),
    request_count INTEGER NOT NULL CHECK (request_count >= 0)
) STRICT, WITHOUT ROWID;
