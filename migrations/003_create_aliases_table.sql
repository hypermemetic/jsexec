-- Function aliases table: named pointers to specific versions
-- Aliases are mutable and can be updated to point to different versions
-- Common aliases: dev, staging, prod
CREATE TABLE IF NOT EXISTS function_aliases (
    function_id TEXT NOT NULL,
    alias TEXT NOT NULL,
    version INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (function_id, alias),
    FOREIGN KEY (function_id, version) REFERENCES function_versions(function_id, version) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_aliases_function_id ON function_aliases(function_id);
CREATE INDEX IF NOT EXISTS idx_aliases_alias ON function_aliases(alias);
