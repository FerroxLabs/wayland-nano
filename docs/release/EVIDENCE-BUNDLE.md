# Release evidence bundle — Wayland Nano (Track B)

A release (owner-gated; never self-approved) is attested by an **immutable
evidence bundle**: a directory of artifacts plus a `MANIFEST.sha256` that
pins the SHA-256 of every file in it. Once sealed, the bundle is append-only —
any change to any artifact invalidates its manifest line and must surface as a
new bundle, not an edit.

## Bundle contents

| Slot | Source | What it proves |
|---|---|---|
| `ci-gate/` | `artifacts/evidence/ci/` — per-leg gate manifests (`<run>-manifest.json`), `sbom.json` (CycloneDX, windows-latest leg), `perf-*.json` | The 6-target gate ran green (fmt/lint/test/deny) on the release SHA; dependency closure is pinned by the SBOM |
| `c12-manifests/` | `scripts/c12-proof/evidence/*.json` | Provisioning / C1.2 proofs on a real Windows host |
| `panel-verdicts/` | `../shared/reviews/panel/` — verdicts + final audits | Cross-model panel review verdicts for the checkpoint under release |
| `canary/` | Canary receipt JSON from `scripts/canary/scan.mjs` | C3.3: the Flux credential appears in no emitted frame/log/session/dump (receipt carries only the key's SHA-256 fingerprint, never the key) |

## Integrity rule

Every file in the bundle — no exceptions — has a line in `MANIFEST.sha256`:

```
<lowercase sha256 hex>  <relative path from bundle root>
```

Verification is `sha256sum -c MANIFEST.sha256` (or the PowerShell equivalent)
from the bundle root. A file not listed, a hash mismatch, or a listed file
missing all fail verification.

## Collecting

`scripts/collect-evidence.ps1` builds a bundle:

```powershell
pwsh nano-k3/scripts/collect-evidence.ps1 -BundleDir .\bundle-v0.1.0-alpha
```

- Copies each slot's artifacts into the bundle, preserving per-slot layout.
- Fails closed: a missing/empty required slot aborts with exit 1 and no
  manifest is written. `-AllowMissing` produces a **partial** bundle stamped
  `"sealed": false` in `bundle.json` — partial bundles are not release
  evidence, only diagnostics.
- Writes `bundle.json` (creation time, source roots, git SHA when available)
  and `MANIFEST.sha256`. The manifest hashes every collected file;
  `bundle.json` itself is listed too.
- Never touches secrets: the canary receipt contains a fingerprint, not the
  key, and the script copies files verbatim without reading contents.

## Owner gate

The bundle is evidence, not approval. Release promotion follows
`../shared/SCORECARD.md`: the owner reviews the bundle (hashes verified),
then promotes or rejects. Agents never seal a bundle into a release
themselves.
