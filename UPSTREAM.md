# Upstream and provenance — Track B (NanoK3)

## Vendored components (copied source, pinned)

| Component | Source | Revision | License | Status |
|---|---|---|---|---|
| `vendor/codex-windows-sandbox-rs` | github.com/openai/codex `codex-rs/windows-sandbox-rs` | `646f7c0a91b8e327d263335da68ae8ef212895ce` | Apache-2.0 + NOTICE | vendored reference; dependency-closure analysis pending before build wiring |
| `vendor/codex-rollout` | github.com/openai/codex `codex-rs/rollout` | same | Apache-2.0 | same |
| `vendor/codex-skills` | github.com/openai/codex `codex-rs/skills` | same | Apache-2.0 | same |

Vendored trees are byte-identical copies (excluding `.git`). Modifications, if
any ever occur, are recorded file-by-file here with rationale. Donor `.git`
metadata is not copied; the immutable donor snapshot lives at
`../resources/upstreams/codex/`.

## Reference-only donors (no code copied)

| Component | Revision | License | Use |
|---|---|---|---|
| Grok Build | `8a14c91d88875a831a38b3a066b1683116bcb31c` | Apache-2.0 + THIRD-PARTY-NOTICES | wire-semantics reference (3-backend sampler); 9 MPL-2.0 packages require review if any code is adapted |
| Kimi Code | `01c74e9372fcbbbe99614e859b53b505ed1664a8` | MIT | behavioral invariants: toolDedupe mechanics, step-retry policy, wire.jsonl journal model |
| Wayland Core 0.12.26 | `98ad1c2836a543385a7a4298f4b3e54a55867ac5` | Apache-2.0 + NOTICE | egress-gate pattern, credential-protection invariants, Flux contract logic to hoist |
| Wayland Desktop beta | `b3cd0511a4406d5e837db9d7e42e395c08387baf` | AGPL-3.0-or-later | protocol fixtures only; no code copying |

## Adapted-file ledger

Empty at scaffold. Every future adaptation records: destination file, donor
path/SHA, license, transformation, notice obligation.
