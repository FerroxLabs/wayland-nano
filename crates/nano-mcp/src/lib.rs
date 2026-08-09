//! nano-mcp — MCP client (stdio, streamable HTTP).
//!
//! Timeouts, cancellation, bounded output, crash containment. Reconnect-once
//! with honestly-documented at-least-once semantics. MCP servers get no
//! security bypass: they run under the sandbox, traffic via nano-egress.
