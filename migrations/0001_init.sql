PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS processed_events (
    event_id TEXT PRIMARY KEY,
    peer_id INTEGER NOT NULL,
    processed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS operations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    peer_id INTEGER NOT NULL,
    author_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('opening', 'income', 'expense', 'cancel')),
    amount INTEGER NOT NULL CHECK (amount >= 0),
    category TEXT,
    target_operation_id INTEGER REFERENCES operations(id),
    cancelled_by_event_id TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_operations_peer_active
    ON operations(peer_id, cancelled_by_event_id, id DESC);
CREATE INDEX IF NOT EXISTS idx_operations_message
    ON operations(peer_id, author_id, message_id);
