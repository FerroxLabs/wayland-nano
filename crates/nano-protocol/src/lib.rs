//! nano-protocol — Desktop wire protocol (versioned, NDJSON stdio).
//!
//! ready-first handshake with capabilities; turn-scoped frames (a frame at
//! least every <600s outside tool windows — Desktop kills quiet turns);
//! stop/ping/pong; clean shutdown on stdin close.
