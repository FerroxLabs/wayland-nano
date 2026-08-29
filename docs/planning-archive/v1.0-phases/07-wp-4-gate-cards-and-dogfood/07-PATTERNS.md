# Phase 7: WP-4 Gate Cards and Dogfood - Pattern Map

**Mapped:** 2026-08-21
**Scope:** builder-owned WP-4 files only
**Authorities:** `07-CONTEXT.md`, `07-RESEARCH.md`, `07-VALIDATION.md`, `SPEC-WP4-gatecards-dogfood.md`, `SPEC-WP-INTERFACES.md`
**Files classified:** 24 logical files/file groups
**Analogs found:** 21 / 24

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog / Authority | Match Quality |
|---|---|---|---|---|
| `gates/README.md` | documentation | request-response / operator flow | `docs/verify/VERIFY-CLI.md`, `docs/verify/CI-ADOPTION.md` | role-match |
| `gates/registry.json` | config / registry | request-response | `crates/nano-verify/src/registry.rs`, `SPEC-WP-INTERFACES.md` §2 | exact consumer contract |
| `gates/lib/card.cjs` | utility / validator | transform | `crates/nano-verify/src/registry.rs::check_inventory`, WP4 §1.2/§6 | behavior-match |
| `gates/lib/contract.cjs` | utility / protocol emitter/parser | transform | `crates/nano-verify/src/gate.rs`, WP4 §1.3/§6 | behavior-match |
| `gates/lib/dirhash.cjs` | utility | recursive file-I/O / transform | WP4 §1.2; `scripts/collect-evidence.ps1` manifest pattern | contract-exact / role-match |
| `gates/lib/artifact-writer.cjs`, `gates/lib/atomic-replace-win32.ps1` | utility / persistence boundary | atomic file-I/O | `SPEC-WP-INTERFACES.md` §9; landed nano-verify atomic receipt writer | contract-exact |
| `gates/fixtures/.gitattributes` | config | file-I/O | repository `.gitattributes`; WP4 §1.1/§1.2 | contract-exact |
| `gates/install-payload/card.md` | model / executable card | transform | WP4 §1.4 complete card | exact authority |
| `gates/install-payload/gate.cjs` | service / gate | file-I/O + subprocess request-response | WP4 §2.1 complete gate; `packaging/npm/scripts/pack.ps1` read-only producer schema | exact authority |
| `gates/install-payload/fixtures/generators/generators.cjs` | utility / generator | batch file-I/O | `packaging/npm/scripts/pack.ps1`; `scripts/soak/soak.mjs` deterministic harness | behavior-match |
| `gates/fixtures/install-payload/{reference,mutants/**}` | test fixture | file-I/O | generator output + WP4 §2.2 mutant matrix | exact authority |
| `gates/config-schema/card.md` | model / executable card | transform | WP4 §§1.2,4.1,6 | exact authority |
| `gates/config-schema/gate.sh` | service / gate | subprocess request-response | WP4 §4.2 complete gate; public `wayland-nano rules` surface | exact authority |
| `gates/config-schema/fixtures/generators/generators.cjs` | utility / generator | batch file-I/O | WP4 §§4.3-4.4; existing Git worktree cleanup in `verify_cmd.rs` | behavior-match |
| `gates/fixtures/config-schema/{probes,mutants/**}` | test fixture / patch corpus | file-I/O | `crates/nano-core/src/execrules.rs`, `crates/nano-cli/src/rules_cmds.rs`, catalog pin test | subject anchors, read-only |
| `gates/provision-script/card.md` | model / executable card | transform | WP4 §§1.2,3.3-3.4,6 | exact authority |
| `gates/provision-script/gate.cjs` | service / gate | file-I/O + subprocess request-response | WP4 §3.6 complete gate; real sandbox serializer | exact authority |
| `gates/provision-script/fixtures/generators/generators.cjs` | utility / generator | batch file-I/O | WP4 §§3.2,3.7; `scripts/soak/soak.mjs` deterministic harness | behavior-match |
| `gates/fixtures/provision-script/{reference,mutants/**}` | test fixture | file-I/O | real dry-run output + WP4 §3.4 mutant matrix | exact authority |
| `gates/tests/gates-card-schema.test.cjs` | test | transform + file-I/O | `registry.rs` tests; `provider_catalog.rs` drift-pin tests | role-match |
| `gates/tests/gates-install-payload.test.cjs` | test | file-I/O + subprocess | WP4 §6; Node assertion style in `scripts/soak/test-budgets.mjs` | behavior-match |
| `gates/tests/gates-config-schema.test.cjs` | test | subprocess + Git worktree lifecycle | `verify_cmd.rs` detached-worktree verification/cleanup | behavior-match |
| `gates/tests/gates-provision-script.test.cjs` | test | file-I/O + subprocess | WP4 §6; external-state oracle style in C1.2 proof | behavior-match |
| `gates/tests/validate-seeded.cjs` | test runner / evidence producer | seeded batch | `scripts/soak/soak.mjs` LCG + manifest | exact role-match |
| `docs/verify/gates.md` | documentation / handoff | request-response | `docs/verify/CI-ADOPTION.md`, WP4 §5.2 | exact role-match |
| `UPSTREAM.md` WP4 rows | provenance ledger | append-only documentation | existing WP3 rows at lines 188-196 | exact |

The fixture directory spelling follows the authoritative WP4 tree: generators remain under each pack, while sealed outputs live under `gates/fixtures/<gate-id>/`. If a plan uses a different layout, it must first reconcile that conflict with WP4 §1.1 rather than silently inventing a third layout.

## Pattern Assignments

### `gates/registry.json` (config, request-response)

**Primary analog:** `crates/nano-verify/src/registry.rs` lines 12-55, 92-157. This is the consuming implementation, so its shape wins over examples.

```rust
pub struct GateClosure {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd_policy: CwdPolicy,
    pub wrapped_tools: Vec<ToolPin>,
}
pub struct GateRegistryEntry {
    pub card: String,
    pub script: String,
    pub closure: GateClosure,
    pub closure_digest: String,
    pub run_artifact: String,
}
pub fn closure_digest(closure: &GateClosure) -> Result<String, VerifyError> {
    let bytes = canonical_json(&serde_json::to_value(closure).map_err(registry_error)?)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
```

Copy these invariants exactly:

- schema remains `1`; populate the existing envelope, do not redesign it;
- each closure has exactly `argv`, `env`, `cwd_policy`, `wrapped_tools`;
- `run_artifact` is a sibling registry field and is never hashed into the closure;
- interpreter shape is `argv[0] = node|bash`, `argv[1] = script` with an exact tool pin;
- card, script, and run artifact are repo-relative existing confined paths;
- requirement mappings are nonempty and cannot dangle;
- emit the file as canonical JSON: UTF-8, byte-lexicographic keys, arrays preserved, integers only, NFC strings, no whitespace or trailing newline.

**Avoid:** copying the bootstrap literal from `verify_cmd.rs`; it is intentionally empty and only supports pre-WP4 startup. Avoid putting the artifact path into `argv` or `closure_digest`.

### `gates/lib/card.cjs` and the three `card.md` files (validator/model, transform)

**Behavior analog:** `crates/nano-verify/src/registry.rs` lines 160 onward for inventory extraction, plus the complete install card in WP4 §1.4. The Node parser is author-time validation; it must agree with the Rust consumer but may not replace it.

Required closed-schema behavior:

- parse only the card's fenced machine block; reject missing, duplicate, or unknown structural fields;
- enforce domain `repo-deliverable`, tier `1`, closed category enum, and ID grammar `^[A-Z]{2,4}-[0-9]{2}$`;
- require a closed six-check inventory per pack, unique IDs, at least five mutants, nonempty `why_fluent`, `expected_drop >= 1`, nonempty `must_fail`, `rotation_k >= 1`, and at least one `gamed_modes` entry;
- validate tool pins, `gate_script_hash`, `last_validated`, reference seal, and every mutant seal;
- recompute every referenced `sealed:dir-sha256:<hex>` before declaring a card valid.

**Avoid:** a general YAML parser dependency or permissive YAML semantics. WP4 adds no package. Do not accept unknown categories/fields, auto-coerce scalars, infer checks from gate output, or treat `last_validated` as meaningful after script-hash drift.

### `gates/lib/contract.cjs` and pack output (utility/service, transform)

**Primary analog:** landed parser behavior in `crates/nano-verify/src/gate.rs`; exact grammar is WP4 lines 127-137 and 741-747.

```text
FAIL <ID> <structure|value|relation|grounding|execution|security>
gate: N/M
```

Implement a small emitter/result helper that accumulates failed inventory IDs and writes only canonical `FAIL` lines followed by exactly one summary. A malfunction emits a deliberately incoherent/bare fail-closed summary such as `gate: 0/M`; it never invents check IDs. Tests must parse stdout using the imported contract and prove no-summary, inconsistent summary, unknown ID, empty stdout, timeout, and spawn errors fail closed.

**Avoid:** PASS lines, free-form evidence, commands, expected values, fixture content, model/provider identity, or exit-code-as-verdict. Do not create a second dogfood parser or runner.

### `gates/lib/dirhash.cjs` (utility, recursive file-I/O)

**Exact authority:** WP4 lines 108-116.

```text
for each recursively enumerated file:
  <NFC relpath with />  <lowercase sha256(file exact bytes)>\n
directory seal = lowercase sha256(concatenated lines)
```

Paths are sorted by UTF-8 byte order after `/` normalization and NFC. Hash exact bytes; never text-decode or normalize CRLF. Reject links/special files and path collisions after normalization. Expose one reusable function for tests/gates and a narrow CLI that prints only the digest.

### `gates/lib/artifact-writer.cjs` (utility, atomic persistence)

Implement the INTERFACES §9 persistence boundary once as an importable Node read/write API plus stdin/stdout CLI so Node, Bash, PowerShell, and integrator evidence paths share identical semantics. Acquire a destination lock with create-new, retry every 50 ms up to 10 s, and break only a recorded lock older than 60 s before re-acquiring. Write and sync a unique same-directory tempfile. Unix uses rename plus parent-directory fsync. Windows must invoke the governed PowerShell helper whose P/Invoke calls `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`; there is no fallback, and Windows explicitly skips unsupported directory fsync. Reads retry exactly once after 100 ms, then return corruption/Unverifiable. Tests prove continuous existing-target visibility, transient and second read failure, contention, crash recovery, sync/replace failure, prior-byte preservation, and residue cleanup. Every WP-4 persistent artifact routes through this helper; scanner receipts and CI/canary scratch are explicitly ephemeral and deleted after their digest/outcome is retained.

**Supporting analog:** `scripts/collect-evidence.ps1` lines 99-109 uses sorted relative `/` paths, lowercase SHA-256, two spaces, and LF for a portable manifest. Copy that manifest discipline, not PowerShell implementation.

### `gates/install-payload/gate.cjs` and generator (service/utility, file-I/O)

**Exact gate authority:** WP4 §2.1.
**Read-only producer analog:** `packaging/npm/scripts/pack.ps1` lines 100-134.

```powershell
$HelperManifest += [ordered]@{
    file = $Helpers[$i]
    size = $HelperInfo.Length
    sha256 = (Get-FileHash ... -Algorithm SHA256).Hash.ToLowerInvariant()
}
$ManifestPlatforms[$Key] = [ordered]@{
    file = $File
    size = $Info.Length
    sha256 = (Get-FileHash ... -Algorithm SHA256).Hash.ToLowerInvariant()
    helpers = $HelperManifest
}
$Manifest = [ordered]@{ schema = 1; algorithm = 'sha256'; platforms = $ManifestPlatforms }
```

The generator stages deterministic reference bytes and derives every mutant from the same reference with exactly one documented fault knob. The gate copies its input to an F-resident private temp directory, re-verifies the card seal before content reads, then checks lifecycle, manifest/tree bijection, primary/helper size+SHA, tamper refusal, wrapper/exec bits, and manifest schema. Use `try/finally` with recursive forced cleanup as prescribed in WP4 §2.1.

**Avoid:** invoking or modifying `pack.ps1`, repairing manifest/tree drift, using the live repo as scratch, or checking only the manifest's self-reported hashes.

### `gates/config-schema/gate.sh`, generator, probes, and patch mutants

**Exact gate authority:** WP4 §4.2. The script is intentionally `set -uo pipefail`, not `set -e`: rejection probes are successful measurements when the CLI exits nonzero. Every other fallible setup action must explicitly route to a fail-closed malfunction.

Use the shipped public CLI as a black box for valid baseline, unknown-field refusal, exact-type/no-coercion, deny fidelity, and byte/token limits. The catalog check must pin the named producer constant; the precedent is `crates/nano-model/tests/provider_catalog.rs` lines 12-43:

```rust
const RECORDED_SHA256: &str = "...";
let mut h = sha2::Sha256::new();
h.update(normalize_eol(VENDORED));
assert_eq!(digest, RECORDED_SHA256,
    "vendored provider catalog drifted — review ... deliberately");
```

Each fluent mutant is a committed unified diff, applied to a fresh detached worktree at the exact base. Build into its own short F-resident `CARGO_TARGET_DIR`, run the unchanged production gate, assert every `must_fail`, and always remove/prune the worktree.

**Cleanup analog:** `crates/nano-cli/src/verify_cmd.rs` lines 634-657:

```rust
let _ = git_success_bounded(repo_root,
    &["worktree", "remove", "--force", &path_text], timeout_ms);
git_success_bounded(repo_root, &["worktree", "prune"], timeout_ms)?;
if worktree.exists() { return Err(()); }
// parse `git worktree list --porcelain` and fail if registration remains
```

**Avoid:** editing/resetting the builder worktree, cleanup only on success, `git clean` against a shared tree, unbounded Cargo target paths, duplicating TOML semantics in Bash, or weakening producer tests to admit a mutant.

### `gates/provision-script/gate.cjs` and generator

**Exact gate authority:** WP4 §§3.2-3.7. Generate the reference packet by invoking the real dry-run serializer and extracting only the marker-framed JSON. The packet arm validates the canonical key set and relations on every platform. The Windows live arm is separate: invoke only the allowed non-elevated probe, require refusal, and prove the exact user/firewall/marker external-state digest is identical before and after.

Mutants are deterministic one-knob transformations of the captured reference: empty identity, donor identity, duplicate path/op key, old version, wildcard uninstall path, and extra elevate key. Preserve packet/live selected-arm exclusivity in output and tests.

**Avoid:** inventing a payload schema, calling the elevated setup path, treating a nonzero exit as sufficient no-mutation evidence, or making the Windows live arm a prerequisite for portable packet tests.

### `gates/tests/*.test.cjs` and `validate-seeded.cjs` (tests/evidence, batch)

Use Node built-ins only: `node:test`, `node:assert/strict`, `node:crypto`, `node:fs`, `node:child_process`. Follow the direct assertion style of `scripts/soak/test-budgets.mjs` rather than a custom mini-framework.

Use one top-level test for each exact required name so discovery count is exactly one. Shared helpers may return structured `{stdout, stderr, status}` but assertions belong in the named tests. References must score M/M; exhaustive mutant tests must enumerate every on-card mutant and assert both `expected_drop` and every `must_fail`.

**Seed pattern:** copy the stable LCG from `scripts/soak/soak.mjs` lines 52-55:

```javascript
let rng = seed >>> 0;
function random() {
  rng = (rng * 1664525 + 1013904223) >>> 0;
  return rng / 2 ** 32;
}
```

Record seed, selected two mutants per pack, observed failures, gate/card/fixture digests, base SHA, and timestamps in a machine-readable manifest. Three seeded runs prove repeatability; they do not replace exhaustive mutant coverage. A mutant scoring M/M is a gate defect and must fail with `GATE_DEFECT <gate_id> <mutant_id>`.

For all temporary resources, allocate beneath validated F-resident `TEMP`/`TMP`, use unique PID/nonce names, and clean in nested `finally` blocks. The canary self-test at `scripts/canary/scan.mjs` lines 77-159 is the JavaScript pattern for nested temporary isolation and unconditional cleanup.

### Dogfood execution and `docs/verify/gates.md`

**Only production entry point:** `crates/nano-cli/src/verify_cmd.rs` lines 803-869.

```rust
let registry = runtime.load_registry(&repo_root, Some(gate_id), None)?;
let artifact = runtime.resolve_artifact(&repo_root, Path::new(&entry.run_artifact))?;
let inventory = runtime.inventory(&repo_root.join(&entry.card))?;
let invocation = GateInvocation { argv: entry.closure.argv, cwd, env, timeout, gate_id };
let outcome = runtime.run_gate(&invocation, &artifact, &inventory).await;
```

Good-tree and deliberate-bad acceptance must call only:

```text
wayland-nano verify --gate <id> --run-only
```

Loop registry keys for the good tree. Materialize ip-m1, pv-m2, and cf-m3 bad trees separately and require Red/FailClosed. Direct script execution is allowed only inside authoring tests and cannot satisfy CARD-08 or mint receipts.

The operator doc should mirror `docs/verify/CI-ADOPTION.md` lines 3-6 and 15-31: document the exact integrator-promoted sibling job, prerequisites, merge blockers, branch-protection note, and ownership boundary. Do not edit `.github/**` in the builder branch.

### `UPSTREAM.md` and final evidence

Follow the exact three-column WP3 ledger pattern at `UPSTREAM.md` lines 186-196:

```markdown
| Destination | Donor | Transformation |
|---|---|---|
| `path` | `exact donor path/revision` or `none — contract section` | Exact copied/adapted behavior and deliberate deviations. |
```

Add one row per adapted file or a precise brace-group only when every member has the same donor and transformation. Distinguish contract-defined originals from donor adaptations; do not claim verbatim reuse where behavior was reimplemented.

Builder evidence should use the manifest disciplines already present in `scripts/soak/soak.mjs` lines 206-217 and `scripts/collect-evidence.ps1` lines 85-109: exact SHA, inputs, counts, selected arms, file hashes, and LF-stable digest inventories. Before handoff, run the exact include-list canary mode; `scripts/canary/scan.mjs` lines 41-74 demonstrates confined inventory, per-file SHA/size, hit count, and a machine-readable receipt without secret bytes.

Integrator-only closure adds no-ff merge SHA/parents, integration gates, `.github` promotion diff, push/fetch equality, and literal status for all six CI legs. The builder must not pre-fill those facts. The external `WP4-FINAL-EVIDENCE.md` is written only after exact-SHA CI is green and must explicitly mark WP-0.1/WP-5/WP-6 unexecuted, then stop.

## Shared Patterns

### Fail Closed

Apply to every library, gate, generator, and test. Missing subject, bad seal, schema skew, unknown field, path escape, spawn failure, timeout, malformed output, cleanup residue, or green mutant is a hard failure. There is no skip for missing pack subject matter. Platform-gated Windows live proof is explicit evidence, not a silent skip disguised as coverage.

### Read-Only Producer Boundary

Apply to all three packs. `packaging/**`, `scripts/provision/**`, `crates/**`, and their config/catalog sources are measurement subjects only. Copy artifacts into private F: scratch or use detached worktrees; never repair or commit producer changes. A verifier defect is routed back to its owner, not shimmed in `gates/lib`.

### Canonicalization

Use two distinct canonical forms and never conflate them:

1. Registry closure JSON: UTF-8, byte-sorted keys, given array order, integers only, NFC strings, compact, no newline.
2. Directory manifest: NFC `/` relative path, two spaces, exact-byte file SHA-256, LF per line, byte-sorted, then SHA-256 of the whole manifest.

### Filesystem and Cleanup

All worktrees, mutation targets, TEMP/TMP, test scratch, generated staging, and evidence scratch resolve on F:. Reject mismatched/noncanonical TEMP/TMP. Use exact owned paths, unique nonces, `try/finally`, worktree remove + prune, directory absence, registry absence, and target absence assertions. Never use C: for project/build/temp material or D: for any WP4 write.

### Evidence Integrity

Evidence is append-only in meaning: capture exact base/head SHA, input hashes, named tests, seeds/selections, reference/mutant scores, cleanup, canary, and command outcomes. Hash retained files and scan only an exact approved inventory. Do not include secret content, free-form gate internals, or future/integrator facts.

## Explicit Reuse / Avoid Matrix

| Concern | Reuse | Avoid |
|---|---|---|
| Card schema | WP4 §1.4 + Rust inventory consumer | permissive YAML, inferred inventory |
| Closure canonical JSON | interface §1 + `registry.rs` canonical digest behavior | ordinary unsorted `JSON.stringify`, artifact in digest |
| Directory hash | WP4 exact line-manifest algorithm | platform hash commands, text normalization |
| Gate protocol | landed WP3 parser semantics | exit-code verdict, PASS/free-form output |
| Generators | deterministic one-fault derivation + soak LCG | hand-edited fixtures, nondeterministic randomness |
| Mutant proof | exhaustive all-mutant tests plus three seeded rotations | sampling as exhaustive proof |
| Config mutants | detached exact-base worktree per mutant | patching/resetting shared checkout |
| Dogfood | WP3 `verify --gate ... --run-only` | direct scripts as acceptance |
| Cleanup | `finally`, remove/prune, filesystem + registration assertions | best-effort cleanup without proof |
| CI promotion | documented snippet, integrator commit after green evidence | builder `.github/**` edit or self-required check |
| Provenance | exact WP3 UPSTREAM rows | vague “inspired by” rows |
| Final evidence | exact-SHA manifest + literal six-leg statuses | early landed claims or starting WP5/6 |

## No Analog Found

| File / Concern | Role | Data Flow | Reason / Planner Direction |
|---|---|---|---|
| `gates/lib/card.cjs` implementation syntax | utility | transform | No existing Node card/YAML parser. Implement only the narrow WP4 machine-block grammar from the authority; add no dependency. |
| `gates/lib/dirhash.cjs` implementation syntax | utility | recursive file-I/O | No existing JavaScript implementation of the exact WP4 directory-seal algorithm. Implement the normative algorithm and cross-platform test vector directly. |
| Integrator `.github` dogfood job | CI config | batch | Intentionally absent and outside builder ownership. Builder writes the documented snippet only; integrator promotes it after evidence is green. |

## Planner Sequencing Guidance

1. Shared libraries, empty-to-populated registry skeleton, byte-exact fixture attribute, and schema/closure/meta tests.
2. Install pack end-to-end to prove the harness cheaply.
3. Config pack with detached-worktree mutation and cleanup proof.
4. Provision packet pack, then Windows live no-mutation arm.
5. Deterministic seal freeze: regenerate, recompute all directory/script/closure pins, update dates, rerun all tests.
6. Seeded runner, good/bad WP3-only dogfood, operator/integrator docs, provenance, builder evidence/canary.
7. Independent audit and at most one fix round; builder stops before merge/push/`.github`.
8. Integrator no-ff promotion, local re-gates, dedicated workflow promotion, push/fetch proof, literal six-leg exact-SHA CI, final external evidence, terminal stop.

Do not split registry data, card pins, generated fixture bytes, and their tests across concurrently edited plans: they are one hash-coupled unit. Pack implementations can be parallel only after shared library contracts are frozen, and each pack must own disjoint directories.

## Metadata

**Analog search scope:** `gates/`, `crates/nano-verify/`, `crates/nano-cli/`, `packaging/npm/`, `crates/nano-sandbox/`, `crates/nano-core/`, `crates/nano-model/`, `scripts/`, `docs/verify/`, `.github/workflows/`, `UPSTREAM.md`, external WP4/interface specs.
**Strong analogs read:** registry canonicalization/confinement, WP3 run-only invocation, detached worktree cleanup, deterministic Node seed/evidence, canary exact-list cleanup/receipt, npm manifest producer, catalog drift pin, CI ownership/promotion, provenance rows.
**Pattern extraction date:** 2026-08-21
