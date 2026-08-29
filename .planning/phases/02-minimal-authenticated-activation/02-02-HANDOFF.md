# Plan 02-02 continuation handoff

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

## Three attempts

1. Verifier failed because npm's exact-version endpoint has no package-level `time` map.
   - Root cause: publication time belongs to the full packument.
   - Fix: query the full packument only for `time['2.1.0']`; retain exact-version metadata for artifact identity.
2. Verifier failed because Windows PowerShell 5's .NET lacks static `SHA512.HashData`.
   - Root cause: runtime API compatibility, isolated and proved with `static_hashdata=False` and a 64-byte `SHA512.Create().ComputeHash()` result.
   - Fix: use a disposable SHA-512 provider.
3. Verifier advanced past those checks, then failed with `PropertyNotFoundStrict: The property 'Name' cannot be found on this object`.
   - Root-cause state: highly likely `Get-ObjectKeys` dereferences `.Name` directly on an empty `PSObject.Properties` collection for the deliberately empty dependency/lifecycle receipt objects. This has not been changed or rerun because strike 3 is a mandatory stop.

## Exact next edit and command

In `scripts/phase2/Test-CanonicalizePackageReview.ps1`, replace:

```powershell
return @($Object.PSObject.Properties.Name | Sort-Object)
```

with:

```powershell
return @($Object.PSObject.Properties | ForEach-Object { $_.Name } | Sort-Object)
```

Then run exactly once from the planning worktree:

```powershell
powershell -NoProfile -File scripts/phase2/Test-CanonicalizePackageReview.ps1 -ReceiptPath .planning/phases/02-minimal-authenticated-activation/evidence/canonicalize-package-review.json -SchemaPath .planning/phases/02-minimal-authenticated-activation/evidence/canonicalize-package-review.schema.json -RequireOwnerDecision
```

If it passes, independently confirm Desktop status remains clean, create `02-02-SUMMARY.md`, and let the parent integrate planning artifacts. Do not install `canonicalize`; only Plan 02-08 may consume the exact approved artifact.

## Anti-patterns discovered

- Do not assume npm's exact-version document includes packument publication history.
- Do not use modern static hash APIs in a verifier explicitly invoked by Windows PowerShell 5.
- Do not dereference collection properties directly under strict mode when the valid collection can be empty.
- Do not convert owner-directed agent operation into a claim of independent human review.
