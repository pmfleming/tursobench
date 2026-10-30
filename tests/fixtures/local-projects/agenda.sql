CREATE TABLE store_meta(id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL CHECK(revision>=0)) STRICT;
INSERT INTO store_meta VALUES(1,0);
CREATE TABLE sources(id TEXT PRIMARY KEY, writable INTEGER NOT NULL CHECK(writable IN (0,1)), deletable INTEGER NOT NULL CHECK(deletable IN (0,1)), completable INTEGER NOT NULL CHECK(completable IN (0,1))) STRICT;
INSERT INTO sources VALUES('local',1,1,1);
CREATE TABLE items(id TEXT PRIMARY KEY, source_id TEXT NOT NULL REFERENCES sources(id), source_uid TEXT, kind TEXT NOT NULL CHECK(kind IN ('event','task')), revision INTEGER NOT NULL CHECK(revision>0), payload BLOB NOT NULL CHECK(length(payload)<=1048576), UNIQUE(source_id,source_uid)) STRICT;
CREATE INDEX items_source ON items(source_id);
CREATE TABLE operation_receipts(request_id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, payload BLOB NOT NULL CHECK(length(payload)<=1048576)) STRICT;
CREATE TABLE migrations(name TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, payload BLOB NOT NULL) STRICT;
