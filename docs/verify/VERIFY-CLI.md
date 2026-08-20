# Verify CLI contract

`wayland-nano verify` has three mutually exclusive modes. The command surface is
closed; combinations not shown here are usage errors.

```text
wayland-nano verify --requirement <id> [--gate <gate-id>] [--task <text>]
                    [--budget <calls>] --cheap-model <id>
                    --escalation-model <id> [--escalation-model <id> ...]
                    [--deadline-ms <milliseconds>] [--receipt-out <path>] [--json]
wayland-nano verify --verify-receipt <path> [--json]
wayland-nano verify --gate <gate-id> --run-only
                    [--deadline-ms <milliseconds>] [--json]
```

## Parsing and defaults

- Minting requires one non-empty `--requirement`, exactly one non-empty
  `--cheap-model`, and one to four unique, non-empty `--escalation-model`
  values. Their occurrence order is the escalation ladder. `--gate` defaults
  to the requirement's registry mapping and `--task` defaults to the registry
  requirement text.
- `--budget` is a nonzero `u32` call count and defaults to 12. Zero, overflow,
  duplicates of a single-use flag, a fifth escalation model, or an unknown
  requirement/gate is exit 2.
- Mint and run-only `--deadline-ms` is a nonzero `u64`, defaults to 600000 ms,
  and is capped at 3600000 ms. It is one absolute monotonic deadline, not a
  fresh timeout per operation. It is invalid with `--verify-receipt`.
- Receipt verification is governed only by `NANO_VERIFY_RECEIPT_BUDGET_MS`:
  120000 ms by default, capped at 600000 ms.
- `--run-only` requires exactly one `--gate` and excludes requirement, task,
  budget, model, receipt-output, and receipt-verification flags. Its registered
  repo-relative artifact is appended to the gate argv; absolute, escaping, or
  missing artifacts are exit 2.
- `--verify-receipt` excludes every mode flag other than `--json`. An absent or
  unreadable receipt path is exit 2; parse/schema failures are an unverifiable
  receipt and exit 6.
- Minting always emits JSONL. `--json` is an accepted no-op there, requests one
  closed result object for run-only, and requests one verdict object for
  receipt verification.

## Exit matrix

| Exit | Meaning |
| ---: | --- |
| 0 | Green (`--run-only`), successfully minted receipt, or `valid` receipt verdict |
| 1 | Engine/runtime failure during a climb |
| 2 | Usage error, including invalid flags, unknown gate, invalid artifact, unreadable receipt path, or non-Git mint cwd |
| 3 | Red/fail-closed run-only gate or climb ending with failing checks |
| 6 | Invalid/tampered receipt: `never-red`, `fabricated-commit`, `gate-mismatch`, `ancestry-unproven`, or `unverifiable` |

CI treats every nonzero value in the `0/1/2/3/6` matrix as failure. Receipt
verification itself returns only 0, 2, or 6.

## JSONL v1 and identifier boundary

Minting writes one JSON object per stdout line. Every frame has `v: 1`, a
`session_id` of the form `wayland-nano-verify-<nanos>`, and a process-local
monotonic `seq` starting at zero. The closed event vocabulary is:

- `verify_started`
- `check_verdict` (`id`, closed lowercase category, and `passed` only)
- `climb_update` (phase, score, accepted, and closed log code only)
- `apply_started` (gate id and closed code)
- `apply_verified` (gate id, changed-file count, and closed code)
- `receipt_minted` (trusted receipt identity after the coherent green rerun)
- `verify_completed`
- `error`

Diagnostics go only to stderr. Frames never expose gate commands or argv,
source, fixtures, diffs, provider/model identity, or free-form provider text.
This identifiers-only boundary applies to success and failure output.

Receipt mode emits no stream. With `--json` it emits exactly one line:

```json
{"schema":"nano.receipt-verdict/1","decision":"valid|never-red|fabricated-commit|gate-mismatch|ancestry-unproven|unverifiable|unknown","requirement":"...","fix_commit":"...","re_derived":true}
```

## Receipt honesty and detached rerun

Offline checking distrusts the receipt, validates red evidence and Git
ancestry, recomputes the registry closure pin, and creates a temporary detached
worktree at `fix_commit`. It reconstructs and runs the pinned gate there within
the receipt budget, then removes the worktree on every outcome. Green is
`valid`; timeout/probe failure is `unverifiable`; red or closure/script drift is
`gate-mismatch`.

The `log_digest` is provenance, not proof. The original log is not retained, so
well-formed digest content cannot be recomputed or authenticated offline. The
offline verifier can reject only an empty or malformed digest; it must never
claim that a structurally valid digest proves the log's contents.

Mint and run-only require the repository and all temporary/build material to be
on F:. `TEMP` and `TMP` must resolve canonically to the same F:-resident root.
Receipt verification likewise uses an F:-resident temporary detached worktree.
No credential or network access is required for receipt verification.
