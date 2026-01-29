-- Functions table: stores mutable function configuration
-- This is the $LATEST version that can be updated
CREATE TABLE IF NOT EXISTS functions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    runtime TEXT NOT NULL DEFAULT 'javascript',
    handler TEXT NOT NULL,
    code TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    environment_variables TEXT, -- JSON serialized HashMap<String, String>
    timeout_ms INTEGER NOT NULL DEFAULT 30000,
    memory_mb INTEGER NOT NULL DEFAULT 128,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_functions_name ON functions(name);
CREATE INDEX IF NOT EXISTS idx_functions_created_at ON functions(created_at);
