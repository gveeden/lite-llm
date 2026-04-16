CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    model_id    TEXT NOT NULL,
    title       TEXT,
    created_at  INTEGER NOT NULL,
    last_used   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,
    content     TEXT NOT NULL,
    tool_call   TEXT,
    tool_result TEXT,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tools (
    name        TEXT PRIMARY KEY,
    description TEXT NOT NULL,
    parameters  TEXT NOT NULL,
    handler     TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL
);
