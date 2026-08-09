# C1 metrics — Track B (nano-k3)

Measured 2026-08-09 on the primary truth machine (Win11 Pro, i9-13900KF, Rust
1.95.0 MSVC, release profile: lto=thin, codegen-units=1, strip=symbols).

## C1.4 — dependency hygiene

| Metric | Value | Method |
|---|---|---|
| First-party crates (workspace) | 12 | `cargo metadata` |
| nano-sandbox production closure | **71 crates total: 2 first-party (nano-sandbox + nano-core), 69 third-party** | `cargo tree -p nano-sandbox --edges normal` |
| Codex donor transitive closure avoided | 86–113k lines per vendored crate (B-VND-01) | donor closure analysis |
| Unsafe surface | contained to ported containment modules (token/acl/wfp/process/job/proc_thread_attr/desktop/dpapi) | module audit |

## C1.5 — binary economics

| Artifact | Release size |
|---|---|
| `nanok3-sandbox-setup.exe` | **981,504 bytes (0.94 MB)** |
| `nanok3-command-runner.exe` | **1,055,744 bytes (1.01 MB)** |
| Clean release build time (crate + both bins) | **25.9 s** (32-core host) |

Note the payoff of the port strategy: the donor crate's release binaries carry
the OTel/Statsig and PTY webs; Track B's stay ~1 MB with the full containment
stack intact.

## C1.6 — substrate maintenance cost

- Vendored-crate refresh cost: `vendor/` is byte-identical reference copies at
  `646f7c0a`; refresh = re-copy + re-run B-VND-01 closure diff (measured:
  no vendored crate is in the build, so refresh risk is zero until the ledger
  shows an adapted-file drift).
- Adapted-file surface: full ledger in `UPSTREAM.md` — every ported file with
  donor path, revision, and transformation list.

## Spawn-prep profile (2026-08-10, nanok3-spawn-profile)

| Phase | Cold, donor semantics | Cold, D9 scoped temp |
|---|---|---|
| session ACL rules (Temp-tree propagation) | **73,000 ms** | **~1 ms** |
| unified_exec 3-test suite | ~100 s | **0.14 s** |
| Full spawn (token+SIDs+ACL+spawn) | ~74 s | **~43 ms** |
