# NANO-MODULE-CONTRACT.md — the module contract v1 (2026-08-20)

**Authority:** the P-MOD (Stage 2) contract for what a Nano module IS. First principle,
non-negotiable: **modules propose; the kernel disposes.** No module may touch journal
admission, egress, containment, approval ordering, or policy monotonicity. Where a lane
disagrees, this file wins. Version 1.0 (draft — unsigned). Unlocks after P-PROF.

**Lineage:** dsh's "everything is a plugin" validated the demand; our audits rejected
Cordis-as-TCB. This contract is the proof-heavy answer: composability WITHOUT giving
modules enforcement authority.

---

## 1. The five frozen seams (enumerated, versioned)

| # | Seam | Type / location (origin/master) | Rule |
|---|---|---|---|
| 1 | Server spec source | `SpecSource` — `crates/nano-agent/src/mcp.rs:348` | Every registered tool/server carries its source |
| 2 | Tool provenance | `ToolSourceId` — `mcp.rs:363` | Stable, typed, never display-name-keyed |
| 3 | Registration receipt | `McpRegistrationReceipt` — `mcp.rs:373-375` | Every registration is receipted + journaled |
| 4 | Instance identity | `mint_instance_id` → `srv_<16-hex-sha256>` — `mcp.rs:401-415` | Content-derived, stable across restarts |
| 5 | Error taxonomy | `NanoErrorKind` + generated table — `crates/nano-session/src/error_codes.rs` | Module errors map to kinds; never ad-hoc strings |

Versioning: the seams carry `contract_version: 1`; additions are append-only; a module
built for v1 must work against v1.x; breaking changes require v2 with a documented
migration. Unknown fields in module manifests are hard errors.

## 2. What a module is (v1)

A directory containing `module.toml` (manifest) + payload. Kinds v1: **MCP-server
module** (registers through the existing MCP client machinery — containment, egress,
approval posture apply identically) and **skill module** (SKILL.md, through
nano-skills). WASM/subprocess-isolated modules are v2.

Manifest (exact, `deny_unknown_fields`):

```toml
contract_version = 1
name = "my-module"                 # strict name grammar (profiles contract grammar)
version = "0.1.0"                  # semver
kind = "mcp-server"                # | "skill"
description = "..."
[source]                           # provenance is mandatory
kind = "registry"                  # | "path"
registry = "wayland"               # registry name
digest = "sha256:..."              # content pin — install verifies or refuses
[[provides]]                       # what it registers
kind = "mcp_tools"
server = { command = "...", args = [...] }
```

## 3. Registration & lifecycle

`wayland-nano plugin add <registry-or-path>` → preview (what it provides, what it costs,
what it can reach) → **always-prompt install with source shown** (unsigned v1) →
install recorded (journaled receipt with the digest) → activation through the five
seams. `remove` is journaled. A corrupt module store is a typed startup refusal —
fail-closed, never a silent zero (the wave-audit lesson).

## 4. The trust boundary (the whole point)

Modules NEVER: decide egress policy, construct sandbox profiles, order approvals,
write the journal directly (they go through the journaled tool-call path), mint
identities, or touch the error taxonomy. A module that tries fails closed with a typed
error. The kernel's reference monitor (nano-egress, nano-sandbox, nano-session,
nano-core policy) is the only enforcement authority — modules are proposals the
kernel disposes.

## 5. Acceptance (when P-MOD builds)

- A third-party-style MCP module installs + activates WITHOUT touching kernel code
  (the proof the contract is real).
- A module with a tampered digest fails closed at install.
- A module attempting an out-of-bounds capability (e.g. spawning outside containment)
  is refused with a typed error.
- The registry-pull path works against a fixture registry with a recorded manifest.
- `just gate-all` green; UPSTREAM.md provenance for any donor-derived machinery.

## 6. Anti-scope (v1)

WASM/plugin sandboxing · marketplace UI · signing/identity beyond digest pins ·
paid/accounts · plugin-authored hooks that run BEFORE policy (hooks are v1 only in
their existing post-policy position) · any module that carries executable code outside
the MCP/skill kinds.

## 7. Signature

**Owner:** ______________ (date)
