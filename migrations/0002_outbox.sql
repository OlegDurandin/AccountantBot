CREATE TABLE IF NOT EXISTS outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    peer_id INTEGER NOT NULL,
    response_type TEXT NOT NULL CHECK (response_type IN ('text', 'budget_chart', 'excel')),
    payload TEXT NOT NULL,
    keyboard TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    available_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_error TEXT,
    sent_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_outbox_pending
    ON outbox(sent_at, available_at, id);
