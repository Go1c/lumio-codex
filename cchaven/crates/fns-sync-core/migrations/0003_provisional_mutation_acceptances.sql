CREATE TABLE provisional_mutation_acceptances (
    origin_client_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    accepted_json BLOB NOT NULL,
    accepted_digest BLOB NOT NULL CHECK(length(accepted_digest) = 32),
    PRIMARY KEY (origin_client_id, operation_id),
    FOREIGN KEY (origin_client_id, operation_id)
        REFERENCES applied_operations(origin_client_id, operation_id)
        ON DELETE CASCADE
);
CREATE INDEX applied_operations_revision_lookup ON applied_operations(revision);
CREATE INDEX provisional_mutation_acceptances_revision_lookup
    ON provisional_mutation_acceptances(revision);
PRAGMA user_version = 3;
