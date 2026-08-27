# Source Ratification Preflight

## Attempt Record

- Attempt 1: The preflight command failed before hashing because PowerShell parsed the pipeline following the `foreach` loop as an empty pipe element; no source/snapshot comparison ran.
- Root-cause hypothesis before attempt 2: accumulating comparison objects in an explicit array before formatting will remove the invalid pipeline parse while leaving the hash inputs unchanged.

Attempt 2 succeeded. No third attempt was required.

## Execution Identity

- UTC timestamp: `2026-08-27T08:27:20Z`
- Planning branch: `plan/persistent-agent-program`
- Planning SHA before artifact creation: `539a3f0f565643f3351877cd631cc594c516789b`
- Hash algorithm: SHA-256 via PowerShell `Get-FileHash -Algorithm SHA256`
- Equality criterion: identical SHA-256 digests for the authoritative source and staged snapshot (the task's reproducible byte-equality test)

## Source-Signature Manifest Inputs

| Artifact | Authoritative source | Staged snapshot | Declared version | Represented signature state | Disposition input | Source SHA-256 | Snapshot SHA-256 | Byte-equal |
|---|---|---|---|---|---|---|---|---|
| NANO-PROGRAM-PLAN | `D:/Development/waylandnano/shared/reviews/research-0.2/NANO-PROGRAM-PLAN.md` | `.planning/sources/NANO-PROGRAM-PLAN.md` | Governing plan dated 2026-08-26; no numeric version stated | Governing, pending owner sign-off | Supersede only the conflicts enumerated by the new signed amendment; retain all non-conflicting P-MEM-1 requirements and history | `50BFD1111C08D8E7C87BA60609B681186D7D858DA602A4F2ACE336340FA7F6E0` | `50BFD1111C08D8E7C87BA60609B681186D7D858DA602A4F2ACE336340FA7F6E0` | YES |
| MEMORY-CONTRACT | `D:/Development/waylandnano/shared/reviews/research-0.2/specs/MEMORY-CONTRACT.md` | `.planning/sources/MEMORY-CONTRACT.md` | v1.2 | Owner-signed 2026-08-25 | Preserve P-MEM-1 schema, durability, retrieval, mediation, and security rules; amend only explicitly listed authority/continuity conflicts | `2B2DAB9CEF77D48683976DEEF9FF6B4A72DAF0529B8F93B18787777F85BC2F82` | `2B2DAB9CEF77D48683976DEEF9FF6B4A72DAF0529B8F93B18787777F85BC2F82` | YES |
| PROFILES-CONTRACT | `D:/Development/waylandnano/shared/reviews/research-0.2/specs/PROFILES-CONTRACT.md` | `.planning/sources/PROFILES-CONTRACT.md` | v1.0 draft | Unsigned draft | Desktop owns product profiles/personas; Nano consumes only enforceable narrowings and pinned references required at its boundary | `CFAFDEE376B7858CC7DC8B3AFC9483421E7A6EA7EB16CF82BF3CCD552D3D9740` | `CFAFDEE376B7858CC7DC8B3AFC9483421E7A6EA7EB16CF82BF3CCD552D3D9740` | YES |
| NANO-MODULE-CONTRACT | `D:/Development/waylandnano/shared/reviews/research-0.2/specs/NANO-MODULE-CONTRACT.md` | `.planning/sources/NANO-MODULE-CONTRACT.md` | v1.0 draft | Unsigned draft | Defer composition-digest work until a concrete Nano enforcement consumer exists; preserve the kernel-disposes security principle | `2A6D3326234EA0FA8E37098791811DE15793CA1227982450572989F6B942D156` | `2A6D3326234EA0FA8E37098791811DE15793CA1227982450572989F6B942D156` | YES |

## Reproduction

Run from `D:/Development/waylandnano/wayland-nano/.tmp-wt-agent-program-plan`:

```powershell
$pairs = @(
  @('D:/Development/waylandnano/shared/reviews/research-0.2/NANO-PROGRAM-PLAN.md', '.planning/sources/NANO-PROGRAM-PLAN.md'),
  @('D:/Development/waylandnano/shared/reviews/research-0.2/specs/MEMORY-CONTRACT.md', '.planning/sources/MEMORY-CONTRACT.md'),
  @('D:/Development/waylandnano/shared/reviews/research-0.2/specs/PROFILES-CONTRACT.md', '.planning/sources/PROFILES-CONTRACT.md'),
  @('D:/Development/waylandnano/shared/reviews/research-0.2/specs/NANO-MODULE-CONTRACT.md', '.planning/sources/NANO-MODULE-CONTRACT.md')
)
foreach ($pair in $pairs) {
  $source = (Get-FileHash -Algorithm SHA256 $pair[0]).Hash
  $snapshot = (Get-FileHash -Algorithm SHA256 $pair[1]).Hash
  if ($source -ne $snapshot) { throw "Source/snapshot mismatch: $($pair[0])" }
}
```

Result: all four pairs matched. No path under `.secrets` was accessed.
