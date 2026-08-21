# Phase 7: WP-4 Gate Cards and Dogfood - Research

**Researched:** 2026-08-21
**Domain:** Sealed executable quality gates, mutation validation, verifier dogfooding, and final program evidence
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- D-01: Implement only the three WP-4 Gate Card packs, their sealed fixtures/mutants,
  registry population, dogfood proof, owned documentation/provenance, and final program evidence.
- D-02: Preserve exact WP-4 ownership. Producer sources being verified remain read-only;
  `.github/**` promotion is integrator-owned after all WP-4 mutant evidence is green.
- D-03: Every pack must have closed inventory/categories/pins, canonical closure and fixture
  digests, at least five fluent-but-wrong mutants, seeded repeatability, and fail-closed meta-tests.
- D-04: Start at exact green master `05637086c81e88550edb002a916a80aff4b278dc`
  in F-only `.tmp-wt-vc-wp-4` on `feat/wp-4`; builder does not merge or push.
- D-05: Promotion remains one Critical/High audit, at most one fix round, local full gate,
  detached no-ff integration, push/fetch proof, exact-SHA six-leg CI, final canary-clean evidence,
  then autonomous work stops. WP-0.1, WP-5, WP-6 and all expansion programs remain unexecuted.

### the agent's Discretion

None stated.

### Deferred Ideas (OUT OF SCOPE)

Do not modify packaging, provisioning, config/catalog producer sources. Do not implement WP-5/6,
DeepSeek, memory, profiles, MCP, or external-agent capabilities. Any verifier defect is routed to
its owner rather than shimmed into Gate Cards.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CARD-01 | Populate the registry with canonical closures, digests, invocation shapes, confined artifacts, and mappings for all three packs. | Registry architecture, canonical digest rules, and per-pack closure table below. |
| CARD-02 | Declare closed card inventory/categories/pins/seals/status/gamed modes. | Card schema and fail-closed metadata tests below. |
| CARD-03 | At least five documented fluent-but-wrong mutants per pack, all caught. | Pack mutation matrix and three-run seeded validation protocol below. |
| CARD-04 | References score M/M; seal/protocol/meta defects fail closed. | Validation architecture maps reference, digest, protocol, drift, and green-mutant meta-tests. |
| CARD-05 | Verify actual install payload/helpers without producer edits. | Install pack reads copied `packaging/npm` and independently checks manifest, bytes, lifecycle, tamper refusal, and wrapper. |
| CARD-06 | Verify Windows dry-run payload, idempotence, and no mutation without producer edits. | Provision pack extracts the real serializer output and uses packet plus Windows live arms. |
| CARD-07 | Verify strict config/catalog behavior with sealed source-patch mutants and cleanup. | Config pack black-boxes the shipped CLI in detached throwaway worktrees. |
| CARD-08 | Dogfood only through WP-3, reject bad and accept good. | Registry-driven run-only loop and deliberate-bad/good-tree acceptance topology below. |
| PROV-03 | Record exact donor transformations for every adapted WP-4 file. | Provenance is a dedicated finalization task before audited-byte gates. |
| EVID-01 | Record dependency-ordered WP commits, gates, and CI links. | Final evidence manifest inventory below. |
| EVID-02 | Append canary-clean shared evidence and mark WP-0.1/WP-5/WP-6 unexecuted. | Builder/integrator ownership split and final evidence procedure below. |
| EVID-03 | Stop after WP-4 and hand state to owner. | Explicit terminal step; no later phase may be started. |
</phase_requirements>

## Summary

WP-4 is an additive gate-library phase, not a producer or verifier phase. The implementation belongs in `gates/**`, with operator documentation in `docs/verify/gates.md` and exact adapted-file rows in `UPSTREAM.md`; packaging, provisioning, Rust crates, and `.github/**` are read-only to the builder. [VERIFIED: `SPEC-WP4-gatecards-dogfood.md` ownership header; `07-CONTEXT.md` D-01/D-02] The current tree is correctly based at locked SHA `05637086c81e88550edb002a916a80aff4b278dc`, and `gates/registry.json` is the landed WP-3 empty bootstrap (`{"gates":{},"requirements":{},"schema":1}`), so WP-4 owns population rather than a schema redesign. [VERIFIED: `git rev-parse HEAD`; `gates/registry.json`; `crates/nano-verify/src/registry.rs`]

Build a shared Node-stdlib gate library first, then install-payload, config-schema, and provision-script packs in that dependency order. Each pack has a sealed reference, at least five sealed fluent-but-wrong mutants, a closed six-check inventory, and a canonical registry closure. All reference/mutant validation must use the same production gate bytes, and all dogfood execution must enter through `wayland-nano verify --gate <id> --run-only`; direct script runs are test-authoring mechanics only and are not the CI acceptance lane. [VERIFIED: `SPEC-WP4-gatecards-dogfood.md` §§1, 5-8; `docs/verify/VERIFY-CLI.md`]

**Primary recommendation:** Treat seal construction, registry closure construction, mutation scoring, dogfood, and evidence closure as one auditable chain: deterministic generators → byte seals → card/registry pins → exhaustive mutant tests → three seeded runs → good/bad verifier loops → full local/integration gates → exact-SHA CI → canary-clean owner handoff.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Card parsing, output contract, directory hashing | Gate library (`gates/lib`) | WP-3 verifier parser | Library validates author-time metadata; landed verifier remains runtime authority for closure and output interpretation. [VERIFIED: WP4 spec §§1,6] |
| Registry identities and closures | `gates/registry.json` | `nano-verify` registry loader | WP-4 supplies data; landed WP-3 confines paths and verifies canonical closure digests. [VERIFIED: `registry.rs`:92-165] |
| Install payload verification | Install Gate Card | Read-only `packaging/npm` | Gate copies and inspects the produced tree; it never repairs producer sources. [VERIFIED: WP4 spec §2] |
| Provision semantics | Provision Gate Card | Read-only sandbox dry-run/setup binaries | Packet arm scores sealed payload; Windows live arm extracts real serializer output and measures no mutation. [VERIFIED: WP4 spec §3] |
| Config/catalog strictness | Config Gate Card | Read-only `nano-cli`, `nano-core`, `nano-model` | Gate drives public `rules` behavior and pinned catalog bytes; source mutants exist only in throwaway worktrees. [VERIFIED: WP4 spec §4] |
| Dogfood PR job | Integrator `.github/workflows/gate.yml` | Builder-owned `docs/verify/gates.md` | Builder documents the exact sibling job; integrator alone promotes it after evidence is green. [VERIFIED: WP4 spec §5; CONTEXT D-02] |
| Final evidence | Integrator/owner shared review surface | Builder handoff artifacts | Builder produces canary-clean inputs; integrator appends promoted SHA/CI proof and stops. [VERIFIED: REQUIREMENTS EVID-01..03] |

## Project Constraints (from AGENTS.md)

- Write only in the assigned WP-4 surfaces; producer sources and donor/upstream snapshots are read-only. [VERIFIED: `AGENTS.md` scope and WP4 spec ownership]
- Never read, echo, copy, or embed secret values; reference the Flux test-key path only. Canary-scan captured evidence. [VERIFIED: `AGENTS.md` Secrets/Evidence]
- Fail closed; never weaken a security invariant or test to make a run green. Missing subject matter fails rather than skips. [VERIFIED: `AGENTS.md` Fail-closed security]
- Namespace Nano-created identities and environment variables; do not reintroduce Track A or NanoK3 names. [VERIFIED: `AGENTS.md` Naming]
- Rust is pinned to 1.95.0/edition 2024 and `windows-sys` remains 0.52; WP-4 should add no Rust dependency. [VERIFIED: `AGENTS.md` Toolchain; WP4 spec]
- Completion requires `just gate-all` (fmt, clippy `-D warnings`, workspace tests, generated checks) plus phase-specific external evidence. [VERIFIED: `AGENTS.md`; `Justfile`:25-37]
- Every donor-adapted file needs an exact `UPSTREAM.md` transformation entry. [VERIFIED: `AGENTS.md` Provenance]
- Builder does not commit, merge, push, self-approve, promote `.github`, or edit owner-managed status surfaces unless explicitly assigned. [VERIFIED: `AGENTS.md` Checkpoints; CONTEXT D-02/D-04]
- F-only is a phase invariant: worktree, temp directories, mutation worktrees, and Cargo target directories must resolve on F:. No D: writes and no C: project/build/temp material. [VERIFIED: CONTEXT D-04; `docs/verify/VERIFY-CLI.md` F-only contract]

## Standard Stack

### Core

| Component | Version/pin | Purpose | Why Standard Here |
|-----------|-------------|---------|-------------------|
| Node.js built-ins + `node:test` | Gate card pin: Node 20 | Card parsing, SHA-256, filesystem copies, subprocesses, deterministic generators, tests | Already specified runtime; avoids new dependencies and cross-platform shell/hash variation. [VERIFIED: WP4 spec §§2.5,3.5,6] |
| Bash | Git Bash on Windows CI | Config gate orchestration | Carries no parser logic; black-boxes the shipped Rust CLI and is explicitly supported on Windows runner. [VERIFIED: WP4 spec §4] |
| WP-3 `wayland-nano verify` | Landed at base SHA | Only production dogfood entry point | Resolves registry, confines artifact, reconstructs closure, clears ambient env, and parses canonical gate protocol. [VERIFIED: `VERIFY-CLI.md`; `verify_cmd.rs`; `registry.rs`] |
| Git detached worktrees | System Git | Isolate config source mutants | Prevents mutation of the builder checkout and enables explicit cleanup assertions. [VERIFIED: WP4 spec §4.3] |
| SHA-256 canonical JSON/dirhash | Interface §1 plus WP4 seal delta | Closure, fixture, script, parser-anchor pins | Makes drift independently recomputable and fail closed. [VERIFIED: interface spec §§1-2; WP4 spec §1.2] |

### Supporting

| Tool | Purpose | Use |
|------|---------|-----|
| `cargo build -p nano-cli -p nano-sandbox --bins` | Builds gated CLI and Windows dry-run/setup helpers | Before config/provision live validation and integrator dogfood. [VERIFIED: WP4 spec §5.2] |
| `just gate-all` | Repository-wide promotion gate | After audited bytes in builder and again after no-ff integration. [VERIFIED: ROADMAP Phase 7 Promotion Gate] |
| `scripts/canary/scan.mjs` | Secret canary scan | Scan every retained evidence/log bundle before handoff. [VERIFIED: `AGENTS.md`; existing script path] |

**Installation:** No external package installation or dependency change is required. Node, Bash, Rust/Cargo, Git, and Just are existing project tools. [VERIFIED: repository configuration and WP4 spec]

## Package Legitimacy Audit

Not applicable: WP-4 installs no new npm, Cargo, or other third-party package. Gate code is stdlib-only. [VERIFIED: WP4 spec runner decisions]

## Architecture Patterns

### System Architecture Diagram

```text
deterministic generator / read-only producer
                 |
                 v
  sealed reference + sealed mutant pool
                 |
                 +--> dirhash --> card pins + parser anchors
                 |                  |
                 |                  v
                 +----------> gates/registry.json
                                      |
operator / CI --> wayland-nano verify --gate ID --run-only
                                      |
                                      v
                confined artifact + canonical closure
                                      |
                                      v
                   production gate (same bytes as tests)
                                      |
                       FAIL IDs + exactly one gate: N/M
                                      |
                                      v
                  landed WP-3 canonical parser/verdict
                         | Green              | Red/FailClosed
                         v                    v
                    accept good tree      block bad tree
```

### Recommended Project Structure

```text
gates/
├── README.md
├── registry.json
├── lib/{card,contract,dirhash}.cjs
├── install-payload/{card.md,gate.cjs,fixtures/generators/generators.cjs}
├── provision-script/{card.md,gate.cjs,fixtures/generators/generators.cjs}
├── config-schema/{card.md,gate.sh,fixtures/generators/generators.cjs}
├── fixtures/
│   ├── .gitattributes
│   └── <gate-id>/{reference,mutants/...}
└── tests/{gates-card-schema,gates-install-payload,gates-provision-script,gates-config-schema}.test.cjs
docs/verify/gates.md
UPSTREAM.md
```

This is the authoritative WP4 tree. Do not add a `run-all` script: the registry-driven verifier loop is the runner. [VERIFIED: WP4 spec §§1.1,5.2]

### Pattern 1: Canonical Sealed Directory

Enumerate files recursively, normalize relative paths to NFC with `/`, sort by byte order, hash exact file bytes, concatenate `<relpath>  <file-sha256>\n`, and SHA-256 that exact LF stream. Fixtures use `* -text` so checkout conversion cannot alter the seal. Every gate verifies the seal before consuming content. [VERIFIED: WP4 spec §1.2]

### Pattern 2: Closed Output Protocol

Gate stdout contains zero or more `FAIL <ID> <category>` lines and exactly one `gate: N/M` summary. The card inventory is the full check authority; unknown IDs, missing output, inconsistent totals, timeout, or spawn error fail closed. Exit status is supplementary, never the verdict. [VERIFIED: interface spec §4; WP4 spec §§1.3,6]

### Pattern 3: Production-Path Mutation Testing

References and mutants are scored by the same gate implementation. Each mutant declares `why_fluent`, `expected_drop`, and `must_fail`. A mutant that scores M/M means `GATE_DEFECT`, not mutant success. Sample rotation is reproducible from a recorded seed, while exhaustive tests still catch every pool mutant. [VERIFIED: WP4 spec §§1.4,6,8]

### Pattern 4: Throwaway Source Mutation

For config mutants, create one detached F-resident worktree per patch, apply the committed diff there, build into a per-worktree F-resident `CARGO_TARGET_DIR`, run the gate, and remove the worktree in unconditional cleanup. Compare `git worktree list` before/after and fail if registrations or directories remain. Never reset/clean the main checkout. [VERIFIED: WP4 spec §4.3; CONTEXT F-only]

### Anti-Patterns to Avoid

- **Direct gate CI execution:** bypasses WP-3 closure/path/env/protocol enforcement; use `verify --run-only`. [VERIFIED: WP4 spec §5.2]
- **Producer repair from WP-4:** violates ownership and makes the gate author self-edit its subject; route defects to owners. [VERIFIED: CONTEXT Scope Fence]
- **Fixture secrecy assumptions:** fixtures are committed by design; opacity is enforced at the verifier output boundary. [VERIFIED: WP4 spec §1.2]
- **Unseeded random sampling:** cannot reproduce evidence. Record the seed and selected IDs for each of three runs. [VERIFIED: WP4 spec §8]
- **Relying on exit code alone or PASS lines:** contradicts the canonical parser contract. [VERIFIED: interface §4]
- **Cleanup only on success:** leaks worktree registrations and F: temp trees; cleanup must run on every outcome and be asserted. [VERIFIED: WP4 spec §4.3]
- **Editing `.github` from the builder:** workflow promotion is integrator-owned after mutation evidence is green. [VERIFIED: CONTEXT D-02]

## Pack Ownership and Exact Behavior

| Pack | Read-only subject | Six-check responsibility | Required mutant classes | Runtime/closure |
|------|-------------------|--------------------------|-------------------------|-----------------|
| `install-payload` | `packaging/npm` copied to temp | Lifecycle success; manifest/tree bijection; primary/helper size+SHA; tamper refusal; wrapper/exec bits; manifest schema | swapped binary, truncated manifest, stale hash, missing platform dir, stripped helper exec bit, no-op installer | Node 20; artifact `packaging/npm`; never mutates repo. [VERIFIED: WP4 spec §§2,1.3] |
| `provision-script` | Real dry-run/setup binaries and sealed payload JSON | exact payload shape; Nano identity namespace; unique derived operation log; non-elevated no-mutation; version floor; created-set/wildcard ownership | empty identity, donor identity, duplicate path/op key, old version, wildcard uninstall path, extra elevate key | Node 20; packet artifact is sealed reference payload; Windows live arm extracts marker-framed JSON and runs no-mutation oracle. [VERIFIED: WP4 spec §3] |
| `config-schema` | `wayland-nano rules`, parser behavior, vendored catalog pin | valid baseline; unknown-field rejection; no coercion; deny fidelity; byte/token limits; named catalog hash | removed deny-unknown, coercing exact, hidden deny rows, raised limit, unpinned catalog edit, invalid decision downgrade | Bash; artifact is probes dir; source patches run only in detached worktrees. [VERIFIED: WP4 spec §4] |

Registry closures must have exactly `{argv, env, cwd_policy, wrapped_tools}`; `run_artifact` is appended only at spawn time and excluded from `closure_digest`. Interpreter entries pin `node` or `bash` and place the script at `argv[1]`. [VERIFIED: interface §2; `registry.rs`:53-89,147-165]

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Gate execution/parsing | New runner or output parser | Landed WP-3 `verify --run-only` | Owns confinement, env clearing, deadlines, closure pinning, and canonical fail-closed verdicts. |
| Canonical closure JSON | Ad-hoc stringification | Landed registry canonicalization plus cross-language test vectors | Avoids key-order/digest skew. |
| Config parser | Gate-side TOML semantics | Black-box shipped `wayland-nano rules` | Tests the delivered behavior rather than a duplicate parser. |
| Provision payload | Invented JSON schema | Extract real dry-run serializer output | Prevents drift from the 17-key producer contract. |
| Worktree mutation isolation | Main-tree reset/clean | Detached worktree per mutant | Preserves user/builder state and allows cleanup proof. |
| CI workflow ownership | Builder `.github` edit | Documented snippet plus integrator promotion | Maintains independent control ownership. |

## Common Pitfalls

### Seal Drift Without Pin Drift

**What goes wrong:** fixtures or gate scripts change while card hashes, parser anchors, `last_validated`, or registry closure digest remain stale.
**Avoidance:** generators and pin updates occur in one bounded task; drift tests intentionally edit each surface and demand closed failure. [VERIFIED: WP4 spec §§1.2,6]

### Provision Live Arm Mutates the Host

**What goes wrong:** a supposedly dry validation invokes elevated setup or overlooks side effects.
**Avoidance:** only the non-elevated setup probe is allowed; snapshot the exact user/firewall/marker triple before and after, require nonzero exit and identical digest, and preserve packet-mode CI-independent validation. [VERIFIED: WP4 spec §3.3 PV-04]

### Windows/Bash Path or Temp Leakage

**What goes wrong:** mixed shells place worktrees/targets under C: or D:, or quote paths incorrectly.
**Avoidance:** establish canonical F-resident TEMP/TMP and Cargo roots before tests; derive all throwaway paths beneath them; assert canonical drive and cleanup. [VERIFIED: CONTEXT D-04; landed VERIFY-CLI F-only contract]

### Mutation Sampling Mistaken for Exhaustive Proof

**What goes wrong:** three seeded `rotation_k=2` runs may not cover the entire pool.
**Avoidance:** retain separate exhaustive `t-*-mutants-caught` tests for every mutant; seeded runs demonstrate stable production rotation, not exhaustive coverage. [VERIFIED: WP4 spec §§6,8]

### Final Evidence Written Too Early

**What goes wrong:** builder evidence claims promoted SHA/CI before integration or appends owner surfaces prematurely.
**Avoidance:** builder freezes a canary-clean handoff manifest; integrator appends exact merge/push/fetch/six-leg CI facts to the external summary only after CI is green. [VERIFIED: ROADMAP promotion gate; EVID-01..03]

## Environment Availability

| Dependency | Required By | Available | Version/pin | Fallback |
|------------|-------------|-----------|-------------|----------|
| Git | detached mutant worktrees | Yes | available in worktree; exact version not material | none |
| Node | libraries/generators/tests | Yes | project gate pin Node 20 | none |
| Bash | config gate | Yes | GNU Bash 5.3.9 observed | Git Bash on Windows CI |
| Rust/Cargo | gated binaries/full gate | Yes | repository pins Rust 1.95.0 | none |
| Just | full gate | Yes | `Justfile` present | invoke constituent commands only for diagnosis, not acceptance |
| Windows runner | provision live/no-mutation arm | CI/integrator | `windows-latest` | packet arm for local cross-platform authoring; live proof still required |

**Missing dependencies with no fallback:** None identified locally. The Windows live arm remains an explicit platform-gated acceptance requirement. [VERIFIED: WP4 spec §§3.7,5]

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Node built-in `node:test`; existing Rust workspace gates |
| Config file | none; tests live under `gates/tests/` |
| Quick run command | `node --test gates/tests/` |
| Full suite command | `just gate-all` plus WP4-specific seeded/dogfood loops |

### Phase Requirements → Test Map

| Req | Behavior | Automated evidence |
|-----|----------|--------------------|
| CARD-01 | registry/schema/closure/artifact/mapping exact | `t-registry-closure-digests`; WP-3 run-only loop over registry keys |
| CARD-02 | card closed metadata | `t-card-schema-valid`; `t-gate-hash-drift-voids-validation` |
| CARD-03 | ≥5 mutants and every mutant caught | `t-ip-mutants-caught`, `t-pv-mutants-caught`, `t-cf-mutants-caught`; three recorded seeded runs |
| CARD-04 | references M/M and fail-closed defects | three `t-*-reference-scores-mm`; fixture digest, dirhash, summary, meta-mutant, hash-drift tests |
| CARD-05 | actual packaged payload/helpers | install reference and mutant battery; good/bad verifier loop |
| CARD-06 | payload/idempotence/no mutation | provision packet reference/mutants plus Windows `--live` before/after oracle |
| CARD-07 | strict parser/catalog and cleanup | config reference/mutants; worktree registration/directory cleanup assertion |
| CARD-08 | bad blocked, good accepted through verifier | good-tree all-three registry loop; deliberate bad arms ip-m1, pv-m2, cf-m3 through run-only |
| PROV-03 | exact transformation rows | owned-file inventory vs `UPSTREAM.md` row audit |
| EVID-01..03 | full promotion proof and stop | final manifest validator/manual exact-SHA comparison; canary scan; explicit terminal handoff |

### Required Named Battery

- Card/schema: `t-card-schema-valid`, `t-registry-closure-digests`.
- References: `t-ip-reference-scores-mm`, `t-pv-reference-scores-mm`, `t-cf-reference-scores-mm`.
- Exhaustive mutants: `t-ip-mutants-caught`, `t-pv-mutants-caught`, `t-cf-mutants-caught`.
- Meta/fail-closed: `t-fixture-digest-fails-closed`, `t-dirhash-canonical`, `t-meta-mutant-passing-is-gate-defect`, `t-summary-contract`, `t-gate-hash-drift-voids-validation`.
[VERIFIED: WP4 spec §6]

### Sampling and Gate Cadence

- **Per library/pack task:** focused gate test file plus reference and all owned mutants.
- **Per pack completion:** `node --test gates/tests/`.
- **Seal freeze:** regenerate deterministically; recompute every reference/mutant directory digest; pin script hash, parser anchors, closure digest, and validation date; rerun the complete Node battery.
- **Repeatability:** three consecutive validation runs, each recording seed and selected two mutants per pack; every selected mutant caught. Exhaustive mutant tests remain independently green.
- **Dogfood acceptance:** good tree returns Green for every registry ID; separately materialized ip-m1, pv-m2, and cf-m3 bad trees return Red/FailClosed via WP-3 run-only. Direct gate output is not accepted as dogfood proof.
- **Builder phase gate:** one Critical/High audit, at most one fix round, fix recheck, `node --test gates/tests/`, seeded runs, good/bad loops, `just gate-all`, provenance inventory, cleanup assertion, canary scan.
- **Integrator gate:** no-ff merge in detached F: worktree, repeat `just gate-all` and dogfood, integrator promotes `.github` job, pushes/fetches exact SHA, then requires literal six-leg CI success.
[VERIFIED: ROADMAP Phase 7; CONTEXT D-05; WP4 spec §8]

### Wave 0 Gaps

- `gates/lib/{card,contract,dirhash}.cjs` do not yet exist.
- Three cards, three gates, three deterministic generators, sealed fixtures/mutants, and four Node test files do not yet exist.
- `gates/registry.json` is intentionally empty and must be populated without changing schema.
- `docs/verify/gates.md` does not yet exist.
- The integrator-promoted `.github` dogfood job is intentionally absent from builder ownership.
[VERIFIED: `rg --files gates`; current registry; WP4 authoritative tree]

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | No | No authentication surface in offline gate packs. |
| V3 Session Management | No | No sessions. |
| V4 Access Control | Yes | Strict file ownership fence, repo confinement, read-only producer treatment, integrator-owned CI promotion. |
| V5 Input Validation | Yes | Closed card schema, deny-unknown registry, confined paths, exact fixture/closure digests, closed FAIL/category grammar. |
| V6 Cryptography | Yes | Standard SHA-256 via Node/Rust libraries; no custom cryptography. |
| V8 Data Protection | Yes | No credentials in gate output/evidence; identifiers-only verifier surface; canary scanning. |
| V12 File and Resource Verification | Yes | Exact byte seals, size/hash verification, no-follow/repo-confinement inherited from WP-3, cleanup assertions. |

### Known Threat Patterns

| Pattern | STRIDE | Mitigation |
|---------|--------|------------|
| Registry/script/fixture drift | Tampering | Canonical closure, script, directory, and parser-anchor digests; drift meta-tests. |
| Fluent malicious or careless producer edit | Tampering | Sealed mutant pools, must-fail IDs, exhaustive plus seeded mutation scoring. |
| Gate reports success without evidence | Spoofing | Canonical inventory reconstruction; inconsistent/missing/unknown output fails closed; exit code non-authoritative. |
| Gate leaks commands/fixtures/secrets | Information disclosure | Dogfood only through WP-3 identifiers-only surface; canary-scan retained logs. |
| Provision validation changes host | Elevation/Tampering | Non-elevated invocation plus before/after external-state digest; no producer edits. |
| Patch escapes or leaves mutated checkout | Tampering/DoS | Confined committed diffs, detached worktrees, per-mutant targets, unconditional cleanup and inventory assertion. |
| Builder self-approves CI control | Elevation of privilege | `.github` remains integrator-owned; branch protection remains owner-owned. |

## Final Evidence Architecture

The final evidence set must distinguish builder facts from integrator facts. Builder-owned evidence should include base SHA, branch, exact owned-file inventory, generator/seal ledger, all reference scores, exhaustive mutant matrix, three seeds/selections/results, good/bad run-only outcomes, audit/fix disposition, local `just gate-all`, provenance audit, worktree/temp cleanup proof, and canary scan result. Integrator-owned closure adds no-ff merge SHA/parents, integration gates, promoted dogfood workflow diff, push/fetch equality, exact-SHA CI run URL/ID, and literal status for all six legs. [VERIFIED: CONTEXT D-05; ROADMAP Promotion Gate; EVID-01]

Only after those facts exist should the integrator append the final canary-clean summary under `F:/Development/waylandnano/shared/reviews/verified-change/`, explicitly listing WP-0.1 as owner-host-run/unexecuted and WP-5/WP-6 as owner-led/unexecuted. The terminal action is an owner handoff; do not create a Phase 8, branch, plan, worktree, or implementation task. [VERIFIED: EVID-02/EVID-03; CONTEXT D-05]

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| — | None. All implementation recommendations derive from locked local authority and landed source. | — | — |

## Open Questions (RESOLVED)

1. **Integrator workflow promotion representation**
   - What we know: the builder must not edit `.github/**`; the exact sibling job is documented in `docs/verify/gates.md`. [VERIFIED: WP4 spec §5.2]
   - **RESOLVED:** promotion is a dedicated integrator-owned commit after the no-ff WP-4 merge and after all WP-4 mutation/dogfood evidence is green; builder plans never own `.github/**`.

2. **External final-summary filename/schema**
   - What we know: EVID-02 fixes the directory and required content but not a filename or JSON schema. [VERIFIED: REQUIREMENTS EVID-02]
   - **RESOLVED:** the integrator writes `F:/Development/waylandnano/shared/reviews/verified-change/WP4-FINAL-EVIDENCE.md` after promotion evidence is complete; the builder creates only an in-repo handoff ledger and makes no landed claim early.

## Sources

### Primary (HIGH confidence)

- `F:/Development/waylandnano/shared/reviews/research-0.2/specs/SPEC-WP4-gatecards-dogfood.md` — authoritative ownership, tree, pack checks/mutants, dogfood, tests, build order, definition of done.
- `F:/Development/waylandnano/shared/reviews/research-0.2/specs/SPEC-WP-INTERFACES.md` — canonical JSON, registry closure, invocation environment, output protocol, receipt/verdict boundaries.
- `.planning/phases/07-wp-4-gate-cards-and-dogfood/07-CONTEXT.md` — locked decisions and scope fence.
- `.planning/REQUIREMENTS.md` and `.planning/ROADMAP.md` — requirement and promotion contracts.
- `AGENTS.md` — project security, filesystem, provenance, gates, and promotion rules.
- `crates/nano-verify/src/registry.rs`, `crates/nano-cli/src/verify_cmd.rs`, `docs/verify/VERIFY-CLI.md` — landed WP-3 behavior.
- Read-only producer sources cited by the WP4 spec: `packaging/npm/**`, `crates/nano-sandbox/**`, `crates/nano-core/src/execrules.rs`, `crates/nano-cli/src/rules_cmds.rs`, `crates/nano-model/tests/provider_catalog.rs`.

### Secondary (MEDIUM confidence)

- None required; local authoritative contracts are complete.

### Tertiary (LOW confidence)

- None.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — prescribed by the authoritative WP4 and interface specifications and present in the worktree.
- Architecture: HIGH — ownership and runtime boundaries are locked and the landed WP-3 implementation was inspected.
- Pack behavior: HIGH — exact check/mutant inventories are normative in the WP4 spec and producer anchors were confirmed.
- Validation: HIGH — named battery and promotion gates are explicit in spec/roadmap/context.
- Integrator choreography: MEDIUM — ownership and outcome are locked, while exact commit packaging of the `.github` promotion is intentionally integrator discretion.

**Research date:** 2026-08-21
**Valid until:** 2026-09-20, or immediately stale if WP-3 verifier/registry behavior or either external specification changes.
