# Plan 02-06 Summary

## Outcome

Legacy filesystem/T2 memory, Nano cron registration/ticking/firing, and legacy hook execution are quarantined from authenticated Phase-2 ACP, protocol-host, and exec paths. Preserved implementations and journal vocabulary remain available for later migration and deterministic replay; no user data is read, migrated, deleted, or reinterpreted as activation authority.

## Implementation

- Forced legacy `memory_*` and `cronjob` calls fail closed before stores or journals.
- Authenticated ACP omits legacy memory context/tools, cron tools/ticker, and legacy hooks.
- Protocol-host and exec retain the quarantine boundary; cron ticker/fire entry points deny before persistent state opens.
- Legacy `Cron*` and `MemoryWrite*` replay remains deterministic and cannot derive activation authority.

## Evidence

- `activation_quarantine`: 3/3 serial
- `activation_legacy_replay`: 1/1
- `nano-agent`: 308/308 plus activation effect tests
- `nano-session`: 121/121 plus integration/adversarial tests
- `nano-cli` library: 194 passed, 1 live-gated ignored; exact cancel regressions passed
- scoped all-target clippy with `-D warnings`, fmt, and diff check: passed

## Bounded broad-suite finding

The broad `nano-cli` integration run found a stale `p4_review` harness that launched production ACP without activation. Two identical EOF results were isolated before attempt 3 by directly proving production exits 2 with its default-off refusal. Production behavior was not weakened; the obsolete test harness is handled separately.
