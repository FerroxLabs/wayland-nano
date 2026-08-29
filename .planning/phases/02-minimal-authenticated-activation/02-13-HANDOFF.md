# Plan 02-13 continuation

Nano worktree `D:/Development/waylandnano/wayland-nano/.tmp-wt-phase2` is on `feat/p2-minimal-authenticated-activation` at committed HEAD `a405a4d` with Task 2 changes preserved uncommitted.

Completed commits:

- `3470035 feat(02-13): bind authenticated activation enablement`
- `a405a4d docs(02-13): define activation operator lifecycle`

Strike history for `cargo test -p nano-agent --test activation_effects -- --test-threads=1`:

1. Production wrapper referenced undeclared `serde_jcs`; corrected to deterministic `serde_json` over closed records/sorted JSON objects.
2. Integration test lacked direct dev dependencies `ed25519-dalek` and `serde_jcs`; `cargo metadata` isolated that they were locked workspace packages but not direct `nano-agent` dependencies, then direct dev dependencies were added.
3. Test-only namespace collision: imported trait `ed25519_dalek::Signer` conflicts with helper `struct Signer`.

Exact next edit on a fresh run: in `crates/nano-agent/tests/activation_effects.rs`, rename helper `Signer` to `TestReceiptSigner` and update its two construction sites plus its `ReceiptSigner` impl. Do not change production behavior. Then run:

```powershell
cargo test -p nano-agent --test activation_effects -- --test-threads=1
```

If green, commit Task 2 atomically, then run:

```powershell
cargo fmt --all -- --check
cargo clippy -p nano-activation -p nano-agent --all-targets -- -D warnings
cargo test -p nano-activation --test enablement -- --test-threads=1
cargo test -p nano-agent --test activation_effects -- --test-threads=1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/phase2/Test-ActivationOperatorDoc.ps1 -Path docs/activation-operator.md -RequireAllHeadings -RequireExecutableCommandShapes -RequireDefaultOff -RejectSecretExamples
cargo test -p nano-activation --test receipt_offline
```

Scope tripwires remain: no ACP/CLI reader, MCP/task adapter extensions (Plan 02-14), quarantine, Desktop, scheduler, memory runtime, graph, extraction, or cross-project work.
