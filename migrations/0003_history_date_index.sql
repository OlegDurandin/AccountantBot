CREATE INDEX IF NOT EXISTS idx_operations_peer_kind_created_at
    ON operations(peer_id, kind, created_at DESC, id DESC);
