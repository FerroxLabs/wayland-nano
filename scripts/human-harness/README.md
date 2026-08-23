# Human-like acceptance harness — Track B

Drives the real binaries the way a user would, live against Flux. External
oracle: every leg checks real state (files on disk, journal events, rendered
screen), never self-report. Credential stays path-only: set
`FLUX_API_KEY_FILE` to the governed key path; the value is never read.

## Run

```powershell
# exec/CLI legs (defaults to the CargoTarget debug binary):
pwsh -NoProfile -File scripts\human-harness\Invoke-HumanHarness.ps1 [-Bin <path>] [-Root <scratch>]

# interactive TUI leg (ConPty driver; launches nano-tui, types, reads, quits):
pwsh -NoProfile -File scripts\human-harness\Invoke-TuiHarness.ps1 [-Bin <path-to-nano-tui.exe>]
```

## Legs

| Leg | Human behavior | Oracle |
|---|---|---|
| L0 | `--version`; bare run | version string; usage + exit 2 |
| L1 | one-shot question | live Flux turn returns the requested token |
| L2a | write attempt in default mode (non-interactive) | `approval_denied`, no file on disk (fail closed) |
| L2b | same with `full_auto` (the pre-approved human) | file on disk with exact content |
| L3 | session inventory | `sessions` lists without error |
| L4 | kill the process mid-turn | journal has `turn_begin` and no `turn_end` (partial, resumable state) |
| L5 | cost meter | `turn_end` + `usage` present in the session journal |
| TUI | launch, type a prompt, wait for the answer, `/quit` | screen transcript: session ready → turn running → response rendered → clean exit |

## Notes

- Non-interactive `exec` in `default` mode denying writes is the product's
  contract, not a failure: there is no human to approve, so it fails closed.
- L4 races a fast model deliberately: the harness watches the journal for
  `turn_begin` and kills before `turn_end`.
- The TUI leg captures the pseudo-console transcript; on hosts where the
  child attaches stdio differently, the transcript also lands in the caller's
  redirected output — the assertion reads either surface.
