DROP INDEX IF EXISTS idx_outbox_pending;

CREATE TABLE outbox_with_excel (
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

INSERT INTO outbox_with_excel
    (id, event_id, peer_id, response_type, payload, keyboard, attempt_count,
     available_at, last_error, sent_at, created_at)
SELECT id, event_id, peer_id, response_type, payload, keyboard, attempt_count,
       available_at, last_error, sent_at, created_at
FROM outbox;

DROP TABLE outbox;
ALTER TABLE outbox_with_excel RENAME TO outbox;

CREATE INDEX idx_outbox_pending
    ON outbox(sent_at, available_at, id);
