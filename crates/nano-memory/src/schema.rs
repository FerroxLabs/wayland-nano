use rusqlite::Connection;

pub(crate) fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(r#"
        PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL);
        INSERT INTO schema_version(version) SELECT 1 WHERE NOT EXISTS(SELECT 1 FROM schema_version);
        CREATE TABLE IF NOT EXISTS episodes(
          id TEXT PRIMARY KEY, content TEXT NOT NULL, source TEXT NOT NULL, source_product TEXT NOT NULL, valid_from TEXT NOT NULL, valid_to TEXT,
          system_time TEXT NOT NULL, source_trust TEXT NOT NULL CHECK(source_trust IN('User','ToolOutput','ModelInference')),
          project TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT 'main', session_id TEXT NOT NULL DEFAULT '');
        CREATE TABLE IF NOT EXISTS facts(
          id TEXT PRIMARY KEY, subject TEXT NOT NULL, predicate TEXT NOT NULL, object TEXT NOT NULL,
          confidence REAL NOT NULL, source_episode TEXT, superseded_by TEXT,
          valid_from TEXT NOT NULL, valid_to TEXT, system_time TEXT NOT NULL,
          source_trust TEXT NOT NULL CHECK(source_trust IN('User','ToolOutput','ModelInference')),
          project TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT 'main', session_id TEXT NOT NULL DEFAULT '');
        CREATE INDEX IF NOT EXISTS facts_conflict ON facts(project,agent_id,subject,predicate,valid_to);
        CREATE TABLE IF NOT EXISTS decisions(
          id TEXT PRIMARY KEY, summary TEXT NOT NULL, why TEXT NOT NULL, how_to_apply TEXT NOT NULL,
          tags TEXT NOT NULL, source_episode TEXT, valid_from TEXT, valid_to TEXT, system_time TEXT NOT NULL,
          source_trust TEXT NOT NULL CHECK(source_trust IN('User','ToolOutput','ModelInference')),
          project TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT 'main', session_id TEXT NOT NULL DEFAULT '');
        CREATE TABLE IF NOT EXISTS procedures(
          id TEXT PRIMARY KEY, title TEXT NOT NULL, steps TEXT NOT NULL, created_by TEXT NOT NULL,
          valid_from TEXT, valid_to TEXT, system_time TEXT NOT NULL,
          source_trust TEXT NOT NULL CHECK(source_trust IN('User','ToolOutput','ModelInference')),
          project TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT 'main', session_id TEXT NOT NULL DEFAULT '');
        CREATE TABLE IF NOT EXISTS working_spillover(id TEXT PRIMARY KEY, content TEXT NOT NULL,
          system_time TEXT NOT NULL, project TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT 'main');
        CREATE TABLE IF NOT EXISTS retention_control(project TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT 'main',
          episode_cap INTEGER NOT NULL, fact_cap INTEGER NOT NULL, byte_cap INTEGER NOT NULL,
          PRIMARY KEY(project,agent_id));
        CREATE TABLE IF NOT EXISTS kg_nodes(id TEXT PRIMARY KEY, kind TEXT NOT NULL, label TEXT NOT NULL,
          source_trust TEXT NOT NULL CHECK(source_trust IN('User','ToolOutput','ModelInference')),
          project TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT 'main');
        CREATE TABLE IF NOT EXISTS kg_edges(id TEXT PRIMARY KEY, source_id TEXT NOT NULL, target_id TEXT NOT NULL,
          predicate TEXT NOT NULL, source_trust TEXT NOT NULL CHECK(source_trust IN('User','ToolOutput','ModelInference')),
          project TEXT NOT NULL, agent_id TEXT NOT NULL DEFAULT 'main');
        CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(id UNINDEXED, kind UNINDEXED,
          project UNINDEXED, agent_id UNINDEXED, session_id UNINDEXED, text);
        CREATE VIRTUAL TABLE IF NOT EXISTS memory_vec USING vec0(
          +record_id TEXT, project TEXT partition key, agent_id TEXT partition key,
          session_id TEXT partition key, embedding float[384]);
    "#)
}
