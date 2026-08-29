# Plan 02-02 continuation handoff

**Status: RESOLVED on 2026-08-30.** The ASCII-only RFC vector correction passed the exact Windows PowerShell 5 verifier. The independently re-downloaded artifact matched the approved integrity, SHA-256, tag, and peeled commit. The exact dependency was then consumed in the authorized Desktop worktree with frozen-lock, Node/Bun vector, formatting, and typecheck gates passing. See `02-02-SUMMARY.md`.

## Exact completion state

- The exact `canonicalize@2.1.0` receipt, closed schema, and independent redownload verifier are written in the planning worktree.
- No dependency was installed. Desktop `package.json` and `bun.lock` were not changed.
- Live evidence already established:
  - npm exact version `2.1.0`, Apache-2.0, one maintainer, no runtime dependencies, and no lifecycle install hooks;
  - dist integrity `sha512-F705O3xrsUtgt98j7leetNhTWPe+5S72rlL5O4jA1pKqBVQ/dT1O1D6PFxmSXvc0SUOinWS57DKx0I3CHrXJHQ==`;
  - tarball SHA-256 `65b2af82fa95b74300d9ecc83f35d44044365a17aadf9d2d153317a7375d015e`;
  - annotated unsigned source tag `v2.1.0` object `a37b200ed2fe15d630b059908e411eb875fdabd5` resolves to npm `gitHead` commit `7fed74ed8addd9f2fe4b2ea4c1c7caf7b793ead2` and tree `5d7cde442eaf380e9e01878447a4d0b41661c4a0`;
  - all 20 tarball file paths, sizes, and SHA-256 hashes are frozen in the receipt;
  - Nano vector manifest SHA-256 is `45a338652bb0ab38ecee59e4754cdad70a131f4da6e7d225640a4a6b0df6f46d`.
- Owner direction is recorded honestly as `owner-directed-agent-operated`, `same_human_controller=true`, and `independent_human_review=false`.
- Planning worktree branch is `plan/persistent-agent-program`; Nano vector source is commit `3e3057c` in `D:/Development/waylandnano/wayland-nano/.tmp-wt-phase2`; Desktop remains at clean base `a59f8404d736dfc8998916d805bd09920e044414`.

## Fresh continuation attempts

1. After replacing direct `.PSObject.Properties.Name` access, the verifier failed because an empty function result unwraps to `$null` under Windows PowerShell strict mode and `.Count` is unavailable.
   - Root cause: the two empty-object call sites did not materialize the function output with `@(...)` before reading `.Count`.
   - Fix applied: both recorded dependency and lifecycle checks now use `(@(Get-ObjectKeys ...).Count)`.
2. The verifier reached the Node runtime gate, then reported `RFC 8785 UTF-16 property ordering mismatch`.
   - The exact embedded runner was extracted byte-for-byte and run against a fresh tarball under PowerShell 7. It produced identical actual/expected strings and code units (`equal=true`, prefix code units `123,34,92,114,34,58,34,67`). This proves `canonicalize@2.1.0` and the ordering expectation agree when the script text is decoded correctly.
3. The exact mandated `powershell -NoProfile` verifier reproduced the same Node mismatch and stopped the lane.
   - Root cause: `Test-CanonicalizePackageReview.ps1` is UTF-8 without BOM and embeds literal non-ASCII RFC keys/values. Windows PowerShell 5 decodes BOM-less script source through the legacy ANSI code page before writing `verify-runtime.cjs`; PowerShell 7 decodes the same source as UTF-8 and the isolated runner passes. The variable is the host parser/encoding, not the reviewed tarball.

## Exact next edit and command

In `scripts/phase2/Test-CanonicalizePackageReview.ps1`, replace the literal-non-ASCII RFC vector at lines 178-179 with an ASCII-only JavaScript representation using `\u` escapes for every non-ASCII key/value character (including the emoji surrogate pair). Preserve the exact decoded RFC input and expected bytes. Do not add a BOM or change the mandated Windows PowerShell command.

Then begin a fresh strike ledger and run exactly once from the planning worktree:

```powershell
powershell -NoProfile -File scripts/phase2/Test-CanonicalizePackageReview.ps1 -ReceiptPath .planning/phases/02-minimal-authenticated-activation/evidence/canonicalize-package-review.json -SchemaPath .planning/phases/02-minimal-authenticated-activation/evidence/canonicalize-package-review.schema.json -RequireOwnerDecision
```

If it passes, independently confirm the tarball hash, source tag/tree correspondence, CommonJS export, Node/Bun vector results, and that Desktop status remains clean. Then create `02-02-SUMMARY.md` and let the parent integrate planning artifacts. Do not install `canonicalize`; this plan explicitly authorizes only the immutable input receipt and reserves dependency consumption for Plan 02-08.

## Anti-patterns discovered

- Do not assume npm's exact-version document includes packument publication history.
- Do not use modern static hash APIs in a verifier explicitly invoked by Windows PowerShell 5.
- Do not dereference collection properties directly under strict mode when the valid collection can be empty.
- Do not embed non-ASCII test vectors directly in a BOM-less `.ps1` that must execute under Windows PowerShell 5; express the JavaScript source in ASCII escapes so host decoding cannot mutate the vector.
- Do not convert owner-directed agent operation into a claim of independent human review.
