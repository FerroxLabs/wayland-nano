# Plan 02-08 Summary

Desktop's private Nano activation producer seam is committed at `a691929c3` in `D:/Development/waylandnano/desktop/.tmp-wt-phase2` on `feat/nano-activation-boundary`.

- The exact `canonicalize@2.1.0` lock entry remains unchanged at integrity `sha512-F705O3xrsUtgt98j7leetNhTWPe+5S72rlL5O4jA1pKqBVQ/dT1O1D6PFxmSXvc0SUOinWS57DKx0I3CHrXJHQ==`.
- One main-process JCS/Ed25519 producer matches Nano source `1bebbec9183d17883497bca76d42e0fdcea275ea` activation and control signatures byte-for-byte and freezes replay identity across startup retries.
- Product authority comes only from an explicit opaque product-subject/principal/project/issuer binding. Its owner-only atomic store has permanent tombstones; mutable conversation, backend, name, path, and persona fields are rejected rather than inferred.
- Issuer keys persist only as `enc:v1:` OS-credential-store ciphertext. Unavailable custody, file fallback, legacy formats, and Linux `basic_text`/unknown backends fail closed.
- The executable verifier binds canonical regular non-reparse path, SHA-256, size, file identity, source commit, and Cargo.lock SHA-256. Its unforgeable held token rechecks identity and can be consumed exactly once by an immediate synchronous launcher.
- Focused Vitest: 3 files, 11/11 tests passed. Scoped Oxlint: zero warnings/errors. Typecheck, Oxfmt check, and `git diff --check`: passed.
- The repository-wide `bun run test` baseline remains red independently of this module: 566 files failed, predominantly `Platform Services not registered` / Electron installation state, while 911 files and 10,886 tests passed. Per scope discipline no unrelated harness change was made.
- `wl` remains unavailable; authenticated GitHub issue 1201 is open, assigned to FerroxLabs, and labeled `area:desktop-ui`, `needs:desktop`, and `state:in-progress`.
- Existing generated changes to `src/common/types/nano-error-codes.json` and `src/common/types/nanoErrorCodes.ts` were preserved and are not part of Plan 02-08.
