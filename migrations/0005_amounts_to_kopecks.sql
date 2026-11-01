BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS app_migrations (
    name TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

UPDATE outbox AS queued
SET payload = (
    SELECT json_group_array(
        json_set(
            item.value,
            '$.income', json_extract(item.value, '$.income') * 100,
            '$.expenses', json_extract(item.value, '$.expenses') * 100
        )
    )
    FROM json_each(queued.payload) AS item
)
WHERE response_type = 'budget_chart'
  AND json_valid(payload)
  AND NOT EXISTS (
      SELECT 1
      FROM app_migrations
      WHERE name = '0005_amounts_to_kopecks'
  );

UPDATE outbox AS queued
SET payload = (
    SELECT json_group_array(
        json_set(
            item.value,
            '$.income', json_extract(item.value, '$.income') * 100,
            '$.expense', json_extract(item.value, '$.expense') * 100,
            '$.balance', json_extract(item.value, '$.balance') * 100
        )
    )
    FROM json_each(queued.payload) AS item
)
WHERE response_type = 'excel'
  AND json_valid(payload)
  AND NOT EXISTS (
      SELECT 1
      FROM app_migrations
      WHERE name = '0005_amounts_to_kopecks'
  );

UPDATE operations
SET amount = amount * 100
WHERE NOT EXISTS (
    SELECT 1
    FROM app_migrations
    WHERE name = '0005_amounts_to_kopecks'
);

INSERT OR IGNORE INTO app_migrations (name)
VALUES ('0005_amounts_to_kopecks');

COMMIT;
