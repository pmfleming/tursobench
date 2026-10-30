CREATE TABLE notifications (
 history_id INTEGER PRIMARY KEY AUTOINCREMENT,
 session_id INTEGER NOT NULL,
 payload_json TEXT NOT NULL,
 preview_json TEXT NOT NULL,
 app_key TEXT NOT NULL,
 search_text TEXT NOT NULL,
 created_unix_ms INTEGER NOT NULL,
 updated_unix_ms INTEGER NOT NULL,
 closed_unix_ms INTEGER,
 close_reason INTEGER
) STRICT;
CREATE INDEX notifications_active_session ON notifications(session_id) WHERE closed_unix_ms IS NULL;
CREATE INDEX notifications_app ON notifications(app_key);
CREATE INDEX notifications_identity ON notifications(session_id,created_unix_ms);
CREATE TABLE notification_once(event_key TEXT PRIMARY KEY, created_unix_ms INTEGER NOT NULL) STRICT;
CREATE TABLE notification_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
