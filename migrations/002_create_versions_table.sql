-- Function versions table: stores immutable published versions
-- Once published, a version cannot be modified
CREATE TABLE IF NOT EXISTS function_versions (
    function_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    code TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    handler TEXT NOT NULL,
    environment_variables TEXT, -- JSON serialized
    timeout_ms INTEGER NOT NULL,
    memory_mb INTEGER NOT NULL,
    published_at INTEGER NOT NULL,
    PRIMARY KEY (function_id, version),
    FOREIGN KEY (function_id) REFERENCES functions(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_versions_function_id ON function_versions(function_id);
CREATE INDEX IF NOT EXISTS idx_versions_published_at ON function_versions(published_at);
