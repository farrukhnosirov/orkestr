//! Tables owned by the memory engine (data-model §4.19–4.21).
//!
//! FTS5 indexes use external content tables kept in sync by triggers, keyed
//! by an `INTEGER PRIMARY KEY` (stable across `VACUUM`).

use orkestr_storage::Migration;

/// Memory items + `memory_fts`.
pub const MEMORY_ITEMS: Migration = Migration {
    id: "memory_0001_items",
    owner: "memory",
    up: "CREATE TABLE memory_items (
            seq        INTEGER PRIMARY KEY AUTOINCREMENT,
            id         TEXT NOT NULL UNIQUE,
            scope      TEXT NOT NULL CHECK (scope IN ('Global', 'Project', 'Agent')),
            project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
            agent_id   TEXT,
            kind       TEXT NOT NULL,
            title      TEXT NOT NULL,
            body       TEXT NOT NULL,
            tags       TEXT NOT NULL,
            rule_key   TEXT,
            pinned     INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            doc        TEXT NOT NULL,
            CHECK ((scope = 'Project') = (project_id IS NOT NULL)),
            CHECK ((scope = 'Agent') = (agent_id IS NOT NULL))
         );
         CREATE INDEX memory_items_scope ON memory_items(scope, project_id, agent_id, pinned, updated_at);
         CREATE VIRTUAL TABLE memory_fts USING fts5(
            title, body, tags,
            content = 'memory_items', content_rowid = 'seq',
            tokenize = 'unicode61 remove_diacritics 2'
         );
         CREATE TRIGGER memory_items_ai AFTER INSERT ON memory_items BEGIN
            INSERT INTO memory_fts(rowid, title, body, tags) VALUES (new.seq, new.title, new.body, new.tags);
         END;
         CREATE TRIGGER memory_items_ad AFTER DELETE ON memory_items BEGIN
            INSERT INTO memory_fts(memory_fts, rowid, title, body, tags) VALUES ('delete', old.seq, old.title, old.body, old.tags);
         END;
         CREATE TRIGGER memory_items_au AFTER UPDATE ON memory_items BEGIN
            INSERT INTO memory_fts(memory_fts, rowid, title, body, tags) VALUES ('delete', old.seq, old.title, old.body, old.tags);
            INSERT INTO memory_fts(rowid, title, body, tags) VALUES (new.seq, new.title, new.body, new.tags);
         END;",
    down: "DROP TRIGGER IF EXISTS memory_items_au;
           DROP TRIGGER IF EXISTS memory_items_ad;
           DROP TRIGGER IF EXISTS memory_items_ai;
           DROP TABLE IF EXISTS memory_fts;
           DROP INDEX IF EXISTS memory_items_scope;
           DROP TABLE IF EXISTS memory_items;",
};

/// Decision log.
pub const DECISIONS: Migration = Migration {
    id: "memory_0002_decisions",
    owner: "memory",
    up: "CREATE TABLE decisions (
            id         TEXT PRIMARY KEY NOT NULL,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            number     INTEGER NOT NULL,
            status     TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            doc        TEXT NOT NULL,
            UNIQUE (project_id, number)
         );
         CREATE INDEX decisions_project ON decisions(project_id, created_at);",
    down: "DROP INDEX IF EXISTS decisions_project; DROP TABLE IF EXISTS decisions;",
};

/// Knowledge sources, chunks + `knowledge_fts`.
pub const KNOWLEDGE: Migration = Migration {
    id: "memory_0003_knowledge",
    owner: "memory",
    up: "CREATE TABLE knowledge_sources (
            id         TEXT PRIMARY KEY NOT NULL,
            project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
            name       TEXT NOT NULL,
            created_at TEXT NOT NULL,
            doc        TEXT NOT NULL
         );
         CREATE INDEX knowledge_sources_project ON knowledge_sources(project_id, created_at);
         CREATE TABLE knowledge_chunks (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            source_id    TEXT NOT NULL REFERENCES knowledge_sources(id) ON DELETE CASCADE,
            path         TEXT NOT NULL,
            ordinal      INTEGER NOT NULL,
            heading_path TEXT NOT NULL,
            text         TEXT NOT NULL,
            token_count  INTEGER NOT NULL
         );
         CREATE INDEX knowledge_chunks_source ON knowledge_chunks(source_id);
         CREATE VIRTUAL TABLE knowledge_fts USING fts5(
            text, heading_path, path,
            content = 'knowledge_chunks', content_rowid = 'id',
            tokenize = 'unicode61 remove_diacritics 2'
         );
         CREATE TRIGGER knowledge_chunks_ai AFTER INSERT ON knowledge_chunks BEGIN
            INSERT INTO knowledge_fts(rowid, text, heading_path, path) VALUES (new.id, new.text, new.heading_path, new.path);
         END;
         CREATE TRIGGER knowledge_chunks_ad AFTER DELETE ON knowledge_chunks BEGIN
            INSERT INTO knowledge_fts(knowledge_fts, rowid, text, heading_path, path) VALUES ('delete', old.id, old.text, old.heading_path, old.path);
         END;
         CREATE TRIGGER knowledge_chunks_au AFTER UPDATE ON knowledge_chunks BEGIN
            INSERT INTO knowledge_fts(knowledge_fts, rowid, text, heading_path, path) VALUES ('delete', old.id, old.text, old.heading_path, old.path);
            INSERT INTO knowledge_fts(rowid, text, heading_path, path) VALUES (new.id, new.text, new.heading_path, new.path);
         END;",
    down: "DROP TRIGGER IF EXISTS knowledge_chunks_au;
           DROP TRIGGER IF EXISTS knowledge_chunks_ad;
           DROP TRIGGER IF EXISTS knowledge_chunks_ai;
           DROP TABLE IF EXISTS knowledge_fts;
           DROP INDEX IF EXISTS knowledge_chunks_source;
           DROP TABLE IF EXISTS knowledge_chunks;
           DROP INDEX IF EXISTS knowledge_sources_project;
           DROP TABLE IF EXISTS knowledge_sources;",
};

/// All memory migrations in order. Must run after the storage migrations.
#[must_use]
pub fn memory_migrations() -> Vec<Migration> {
    vec![MEMORY_ITEMS, DECISIONS, KNOWLEDGE]
}
