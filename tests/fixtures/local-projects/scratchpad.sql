CREATE TABLE session_state(id INTEGER PRIMARY KEY CHECK(id=1), payload BLOB NOT NULL) STRICT;
CREATE TABLE tabs(position INTEGER PRIMARY KEY, payload BLOB NOT NULL) STRICT;
CREATE TABLE buffer_ids(id TEXT PRIMARY KEY) STRICT;
CREATE TABLE history(temp_id TEXT NOT NULL REFERENCES buffer_ids(id) ON DELETE CASCADE,
                     seq INTEGER NOT NULL, global_seq INTEGER NOT NULL CHECK(global_seq>0),
                     entry BLOB NOT NULL, bytes INTEGER NOT NULL DEFAULT 0 CHECK(bytes>=0), PRIMARY KEY(temp_id, seq)) STRICT;
CREATE INDEX history_global_seq ON history(global_seq);
CREATE INDEX history_sizes ON history(temp_id, bytes);
CREATE TABLE snapshots(id TEXT PRIMARY KEY REFERENCES buffer_ids(id) ON DELETE CASCADE,
                       bytes INTEGER NOT NULL, digest BLOB NOT NULL) STRICT;
CREATE TABLE blobs(digest BLOB PRIMARY KEY, body BLOB NOT NULL, bytes INTEGER NOT NULL DEFAULT 0 CHECK(bytes>=0)) STRICT;
CREATE INDEX blob_sizes ON blobs(bytes, digest);
CREATE TABLE parts(temp_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
                   part INTEGER NOT NULL, digest BLOB NOT NULL REFERENCES blobs(digest),
                   PRIMARY KEY(temp_id, part)) STRICT;
CREATE INDEX parts_digest ON parts(digest);

CREATE TABLE quarantine(id TEXT PRIMARY KEY, payload BLOB NOT NULL, reason TEXT NOT NULL) STRICT;

CREATE TABLE closed_tabs(sequence INTEGER PRIMARY KEY, token TEXT NOT NULL UNIQUE, closed_at INTEGER NOT NULL, payload BLOB NOT NULL) STRICT;

CREATE TABLE buffer_versions(id INTEGER PRIMARY KEY AUTOINCREMENT, temp_id TEXT NOT NULL REFERENCES buffer_ids(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL, bytes INTEGER NOT NULL, digest BLOB NOT NULL, source_token TEXT,
    text_digest BLOB, anchor_seq INTEGER, anchor_before INTEGER NOT NULL DEFAULT 0, branch_history BLOB,
    branch_bytes INTEGER NOT NULL DEFAULT 0, history_high_water INTEGER NOT NULL DEFAULT 0) STRICT;
CREATE INDEX version_sequences ON buffer_versions(history_high_water);
CREATE INDEX version_sizes ON buffer_versions(temp_id,bytes,branch_bytes,digest);
CREATE UNIQUE INDEX version_source ON buffer_versions(temp_id, source_token);
CREATE INDEX versions_buffer ON buffer_versions(temp_id, id);
CREATE TABLE version_parts(version_id INTEGER NOT NULL REFERENCES buffer_versions(id) ON DELETE CASCADE,
    part INTEGER NOT NULL, digest BLOB NOT NULL REFERENCES blobs(digest), PRIMARY KEY(version_id, part)) STRICT;
CREATE INDEX version_parts_digest ON version_parts(digest);
