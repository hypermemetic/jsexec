-- Invocations table: stores metrics and logs for each function invocation
CREATE TABLE IF NOT EXISTS invocations (
    invocation_id TEXT PRIMARY KEY,
    function_id TEXT NOT NULL,
    function_name TEXT NOT NULL,
    version INTEGER, -- NULL for $LATEST
    invocation_type TEXT NOT NULL, -- RequestResponse, Event, DryRun
    cold_start INTEGER NOT NULL DEFAULT 0, -- boolean: 0 = warm, 1 = cold
    duration_ms INTEGER,
    memory_used_bytes INTEGER,
    cpu_time_ms INTEGER,
    success INTEGER NOT NULL, -- boolean: 0 = failed, 1 = success
    error_message TEXT,
    error_type TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    FOREIGN KEY (function_id) REFERENCES functions(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_invocations_function_id ON invocations(function_id);
CREATE INDEX IF NOT EXISTS idx_invocations_started_at ON invocations(started_at);
CREATE INDEX IF NOT EXISTS idx_invocations_success ON invocations(success);
CREATE INDEX IF NOT EXISTS idx_invocations_cold_start ON invocations(cold_start);
