# WP-0.1 CUA live desktop proofs (Windows) — owner/host-run

Runs the §7.2 live-gated CUA proofs on an interactive Windows desktop and
records evidence with the binary SHA-256 and toolchain identity. Both proofs
need a real interactive session — that is why this is owner/host-run and why
`computer_use` stays pinned `false` until the evidence exists (honesty rule).

## Run (interactive desktop session)

```powershell
pwsh -NoProfile -File scripts\cua-proof\Test-CuaProof.ps1
```

The probe window appears briefly at (200,200) and receives one synthesized
click. Evidence lands in `scripts/cua-proof/evidence/`.

## What it proves

- `windows_focus_invariance_and_sendinput_landing` — a synthesized click never
  changes the frontmost app, AND the click lands inside the probe window's
  client rect (the helper `nano-cua/examples/cua_probe_window.rs` records the
  raw `WM_LBUTTONDOWN` client coordinates and prints one result JSON line; the
  test parses that external process output, never a self-report).
- `windows_hidpi_coordinate_equivalence` — owner-run twice, once at 100% and
  once at 150% display scale (change the scale between runs); the screenshot
  path must return a decodable PNG with nonzero dimensions on both.

The HiDPI leg changes the host's display scale factor — run it deliberately,
not unattended.
