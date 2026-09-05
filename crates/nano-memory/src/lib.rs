//! Project- and agent-partitioned persistent memory.

mod embed;
mod mediation;
mod resolver;
mod schema;
mod store;
mod types;

pub use embed::{EMBEDDING_DIM, Embedder, HashedEmbedder};
pub use mediation::{MemoryProposal, MemoryReceipt, ProposalKind};
pub use resolver::{ContradictionResolution, ResolverCandidate, resolve_contradiction};
pub use store::{
    LegacyMigrationCompletion, LegacyMigrationError, LegacyMigrationResult, LegacyMigrationWrite,
    MemoryStore, migrate_legacy_facts_with_fault_injection, rebuild_from_journals,
};
pub use types::{
    AgentScope, ConfiguredAgents, DecisionWrite, DeletionRule, EmbedderChoice, EpisodeWrite,
    FactState, FactWrite, MemoryError, MemoryPolicy, MemoryResult, ProcedureWrite, ReadScope,
    RetentionCaps, RetrievalEvidence, RetrievalIdentity, RetrieveHit, RetrieveQuery, SourceTrust,
    WriteScope,
};

use rusqlite::Connection;

pub fn register_sqlite_vec() {
    type EntryPoint = unsafe extern "C" fn(
        *mut rusqlite::ffi::sqlite3,
        *mut *mut std::ffi::c_char,
        *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::ffi::c_int;
    // SAFETY: sqlite-vec exports SQLite's documented extension entrypoint.
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<*const (), EntryPoint>(
            sqlite_vec::sqlite3_vec_init as *const (),
        )))
    };
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    Connection::open_in_memory()
}
