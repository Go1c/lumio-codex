CREATE TABLE applied_operations_v2 (
    origin_client_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    body_digest BLOB NOT NULL CHECK(length(body_digest) = 32),
    receipt_kind TEXT NOT NULL CHECK(receipt_kind IN ('legacy','mutation_result','conflict_resolution')),
    mutation_json BLOB,
    CHECK(
        (receipt_kind = 'mutation_result' AND mutation_json IS NOT NULL)
        OR (receipt_kind IN ('legacy','conflict_resolution') AND mutation_json IS NULL)
    ),
    PRIMARY KEY (origin_client_id, operation_id)
);

INSERT INTO applied_operations_v2 (
    origin_client_id,
    operation_id,
    revision,
    body_digest,
    receipt_kind,
    mutation_json
)
SELECT
    origin_client_id,
    operation_id,
    revision,
    body_digest,
    'legacy',
    NULL
FROM applied_operations;

DROP TABLE applied_operations;
ALTER TABLE applied_operations_v2 RENAME TO applied_operations;
PRAGMA user_version = 2;
