# Notices — Wayland Nano (Track B)

Third-party attribution for the `nano-k3` workspace. This file is release
evidence: keep it in sync with `UPSTREAM.md` (provenance ledger) and
`deny.toml` (license gate).

## Vendored / ported OpenAI Codex code (Apache-2.0)

The following components are derived from
[openai/codex](https://github.com/openai/codex) (`codex-rs`), pinned to commit
`646f7c0a91b8e327d263335da68ae8ef212895ce`, licensed under the Apache License,
Version 2.0. Per the upstream NOTICE:

> OpenAI Codex
> Copyright 2025 OpenAI
>
> This project includes code derived from [Ratatui](https://github.com/ratatui/ratatui), licensed under the MIT license.
> Copyright (c) 2016-2022 Florian Dehau
> Copyright (c) 2023-2025 The Ratatui Developers

The full Apache-2.0 license text ships at `vendor/LICENSE`; the upstream
NOTICE ships at `vendor/NOTICE`. `vendor/codex-windows-sandbox-rs` carries no
LICENSE file of its own — the `vendor/`-level LICENSE/NOTICE pair is the
authoritative copy for the whole vendored tree.

Apache-2.0 grant header (from `vendor/LICENSE`):

> Licensed under the Apache License, Version 2.0 (the "License");
> you may not use this file except in compliance with the License.
> You may obtain a copy of the License at
> http://www.apache.org/licenses/LICENSE-2.0
> Unless required by applicable law or agreed to in writing, software
> distributed under the License is distributed on an "AS IS" BASIS,
> WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

Covered components (file-by-file transformations are recorded in
`UPSTREAM.md`):

- `vendor/codex-windows-sandbox-rs` — vendored reference copy of
  `codex-rs/windows-sandbox-rs` (byte-identical to the pinned revision).
- `vendor/codex-rollout`, `vendor/codex-skills` — vendored reference copies.
- `crates/nano-sandbox` — ported sandbox backends:
  `codex-rs/sandboxing` (seatbelt policy builder + `.sbpl` bases, landlock argv
  builder, bwrap argv builder) and `codex-rs/linux-sandbox` (helper binary:
  bwrap pipeline, legacy landlock/seccomp enforcement, launcher, bundled-bwrap
  loader). Managed-proxy surfaces were deliberately dropped; see `UPSTREAM.md`.
- `crates/nano-tools/src/shell.rs` (unix path) — caller-side unix sandbox
  wiring adapted from `codex-rs/sandboxing/src/manager.rs`.

## Wayland Desktop Core corpus (provenance pin)

`crates/nano-protocol/corpus/wayland-desktop-core/v1/` is a byte-for-byte
mechanical import from Wayland Core commit
`d0aa0abc75afe056cc5434fcd652efa6d474ab0c`
(`FerroxLabs/wayland-core`, branch `origin/feat/887`). Contract
`wayland-desktop-core` `1.0`, generator `wcore-desktop-contract-gen/1`; fixture
/ schema / source-input SHA-256 pins are recorded in that directory's
`PRODUCER-PIN.md`, which is the contract authority for the v1 consumer.

## crates.io dependency classes

Full machine-readable dependency data: the CycloneDX SBOM (`sbom.json`)
emitted by the CI gate (windows-latest leg, see
`.github/workflows/gate.yml`), plus `Cargo.lock`. License allowances are
enforced by `cargo deny check` against `deny.toml`. Summary by class:

- Async runtime: `tokio` (MIT), `async-trait` (MIT/Apache-2.0).
- HTTP/TLS client: `reqwest` + `hyper`/`hyper-util` + `rustls` stack
  (MIT/Apache-2.0/ISC); root certificates via `webpki-roots`
  (CDLA-Permissive-2.0 — Mozilla root store data).
- Serialization: `serde`, `serde_json`, `serde_yaml` (MIT/Apache-2.0);
  `unicode-ident` et al. (Unicode-3.0).
- Error handling: `anyhow`, `thiserror` (MIT/Apache-2.0).
- Windows platform API: `windows` 0.58, `windows-sys` 0.52 (pinned;
  MIT/Apache-2.0).
- Linux sandbox: `landlock`, `seccompiler`, `libc`, `clap`
  (MIT/Apache-2.0/BSD-3-Clause).
- Filesystem/path/glob: `dirs-next`, `dunce`, `glob`, `globset`, `tempfile`
  (MIT/Apache-2.0/BSD).
- Crypto/hashing: `sha2` (MIT/Apache-2.0). Encodings: `base64`.
- Time: `chrono` (MIT/Apache-2.0). Randomness: `rand` (MIT/Apache-2.0).
- Logging: `tracing`, `tracing-appender` (MIT).
- Text matching: `regex`, `regex-lite` (MIT/Apache-2.0).
- Dev-only (never shipped in binaries): `pretty_assertions`, `tempfile`.

No GPL/LGPL/AGPL dependencies; MPL-2.0 is deliberately not allowed (see
`deny.toml`). Duplicate-version warnings (e.g. `syn`, `thiserror`,
`windows-*` impl crates) are accepted for the alpha and tracked by
`cargo deny check bans` output.
