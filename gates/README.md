# Wayland Nano Gate Cards

This directory contains the three closed WP-4 Gate Card packs:
`install-payload`, `provision-script`, and `config-schema`. They verify produced
artifacts; they do not own or repair the packaging, provisioning, configuration,
catalog, Rust, or CI producer surfaces they inspect.

## Trust boundary

`gates/registry.json` is the runtime identity and closure authority. Each card's
machine block declares a closed check inventory, categories, tool pins, fixture
seals, validation state, gamed modes, and escape-hatch bans. Unknown, missing, or
duplicate fields invalidate a card. Gate stdout is intentionally opaque: zero or
more `FAIL <ID> <category>` lines followed by exactly one `gate: N/M` summary.
There are no PASS lines. Missing, unknown, duplicated, or incoherent output fails
closed.

Fixtures are committed under `gates/fixtures/<gate-id>/` and are byte-exact because
`gates/fixtures/.gitattributes` disables text conversion. A fixture seal is
`sealed:dir-sha256:<hex>`. The directory digest recursively hashes exact file bytes,
normalizes relative names to NFC `/` paths, rejects links and special files, sorts
by UTF-8 byte order, and hashes the concatenated lines
`<path>  <file-sha256>\n`. Any byte, path, file-type, or normalization drift voids
the seal. The registry closure is separately serialized as canonical JSON and
SHA-256 pinned. Editing a gate script changes `gate_script_hash` and voids prior
`last_validated` evidence.

## Authoring and validation

1. Work only in an F:-resident isolated checkout with F:-resident scratch paths.
2. Change a pack under `gates/**`; keep `packaging/**`, `scripts/provision/**`,
   `crates/**`, and `.github/**` read-only. Workflow promotion belongs to the
   integrator after every mutant is green.
3. Regenerate the reference and every documented fluent-but-wrong mutant
   deterministically. Persist every generated fixture, seal, card, report, registry,
   and evidence byte with the shared artifact writer described below.
4. Recompute all directory seals, the gate script hash, canonical registry closure,
   and closure digest. Update pins together; never bless drift independently.
5. Run the complete Node meta-test battery. The reference must score M/M and every
   mutant must drop its documented checks. A green mutant emits exactly
   `GATE_DEFECT <gate_id> <mutant_id>` and blocks validation.
6. Run the seeded rotations and the good/bad trees through the landed WP-3 entry
   point only: `wayland-nano verify --gate <gate-id> --run-only`. Directly executing
   a gate helps author tests, but is never dogfood evidence and never mints a receipt.
7. Run `just gate-all`, provenance, ownership, cleanup, and canary checks before
   handing the promotion request to the integrator. Stop at every ownership or
   fail-closed boundary; do not patch producer or verifier defects in a card.

## Canonical artifact writer

All persistent WP-4 writers must import `gates/lib/artifact-writer.cjs` and call
`writeArtifact(target, bytes)`, or pipe exact bytes to:

```text
node gates/lib/artifact-writer.cjs write <target>
node gates/lib/artifact-writer.cjs read <target>
```

The writer acquires `<target>.lock` using create-new semantics (50 ms retries, 10 s
cap), and may break only a lock older than 60 seconds before re-acquiring it. It
writes and syncs a unique same-directory temporary file, atomically replaces the
target, syncs the parent directory on Unix, then removes temporary and lock residue.
Windows replacement is exclusively the governed PowerShell helper's exact
`MoveFileExW(..., MOVEFILE_REPLACE_EXISTING)` call. PowerShell, P/Invoke, sync, or
replacement failure preserves prior bytes and fails closed; there is no copy,
pre-delete, truncate, cross-directory, or rename fallback. Readers retry one failed
read exactly once after 100 ms, then report `ARTIFACT_UNVERIFIABLE`.

Ephemeral test and CI scratch must also stay on F: and be deleted after its validated
digest or outcome has been retained. No project, build, or temporary material belongs
on C: or D:.
