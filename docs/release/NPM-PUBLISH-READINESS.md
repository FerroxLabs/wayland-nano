# NPM publish readiness

The public package is `waylandnano`. On 2026-08-13, a read-only registry query
returned `waylandnano@0.1.0-alpha.0`, repository
`github.com/FerroxLabs/wayland-nano`, with `next` and `latest` pointing at that
version. The name is therefore already registered for this project; the owner
must confirm that the repository's `NPM_TOKEN` can publish it before cutting a
tag. Agents do not inspect, create, or handle that credential.

## Release behavior

- `.github/workflows/release.yml` is the only npm publication workflow.
- It triggers only for explicit `v*` tag pushes, never branch pushes or merges.
- Five native jobs stage and verify `win32-x64`, `darwin-arm64`, `darwin-x64`,
  `linux-x64`, and `linux-arm64`, then emit one artifact per platform.
- The publish job downloads those artifacts and calls `pack.ps1` to assemble
  the full binary tree plus `binaries-manifest.json`.
- The manifest records exact byte size and SHA-256 for every staged binary.
- Prereleases publish to `next`; stable versions publish to `latest`.
- `NPM_TOKEN` is read from GitHub Actions repository secrets as
  `NODE_AUTH_TOKEN` and is never echoed.

## Owner pre-tag checklist

1. Confirm npm account/team write access to `waylandnano`.
2. Confirm the tag version exactly matches `packaging/npm/package.json`.
3. Confirm the protected `NPM_TOKEN` repository secret is current.
4. Review the five platform artifacts and their generated integrity manifest.
5. Approve the explicit version tag; there is no publish-on-merge path.
