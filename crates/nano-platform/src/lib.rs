//! nano-platform — the OS boundary.
//!
//! trait Platform { process_executor, filesystem, sandbox, shell(mode),
//! environment, permissions } with windows/ macos/ linux implementations.
//! No cfg(target_os) outside this crate and nano-sandbox.
