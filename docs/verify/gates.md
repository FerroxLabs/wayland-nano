# Gate-card operation and promotion

The canonical dogfood entry point is the landed local WP-3 binary:

```text
wayland-nano verify --gate <registry-id> --run-only
```

Direct gate scripts, npm-installed verifiers, and receipt flows do not satisfy WP-4 dogfood. Before the install-payload gate runs, stage the complete package with `packaging/npm/scripts/pack.ps1`; generated package binaries and its manifest are ephemeral and must leave the tracked producer diff empty.

## Integrator-owned promotion

The builder stops after local evidence and requests this sibling top-level job be appended to the existing `.github/workflows/gate.yml`. The builder does not edit `.github`, push, configure branch protection, or declare its own check required. `docs/verify/ci/verify-dogfood.yml` remains dormant documentation.

```yaml
  gate-cards:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }
      - uses: actions/setup-node@v4
        with: { node-version: "20" }
      - uses: dtolnay/rust-toolchain@master
        with: { toolchain: "1.95.0" }
      - name: Build the landed verifier and gated helpers
        run: cargo build -p nano-cli -p nano-sandbox --bins
      - name: Stage the sealed npm payload
        shell: pwsh
        run: packaging/npm/scripts/pack.ps1 -Platform all -ArtifactRoot artifacts/npm-binaries
      - name: Dogfood every registered gate through WP-3
        shell: bash
        run: |
          set -euo pipefail
          for gate_id in $(node -e "console.log(Object.keys(require('./gates/registry.json').gates).sort().join(' '))"); do
            ./target/debug/wayland-nano verify --gate "$gate_id" --run-only
          done
```

Prerequisites are the reviewed sealed fixtures, exact registry closure pins, a complete five-platform npm artifact root, and Green builder evidence. A Red or FailClosed outcome, closure/card/fixture drift, missing artifact, staging residue, provenance gap, or canary hit blocks merge. After the dedicated integrator promotion commit passes the literal six-platform matrix plus `gate-cards`, the owner may make that status required on `master`.
