# 03-01 Summary: mem-sec gate-card pack

## Scope and base

- Base: `a879e2fb33964d942f0cbc5634c2736a44939ef7` (`origin/master` at worktree creation).
- Worktree: `D:/Development/waylandnano/wayland-nano/.tmp-wt-p3-gates`.
- Branch: `feat/p3-mem-sec`.
- Anti-scope preserved: no KG pass-3, hosted memory, MCP memory exposure,
  extraction, global reads, schema rename, scheduler, registry, UI, or provider work.

## Commit boundaries

1. Gates-first commit: `875a97b523cd47b76c467a9eef2b7d1e69201712`
   (`test(memory): seal mem-sec gate pack`). It contains the sealed fixtures and
   relevance labels, 30 committed mutant patches, card, gate script, registry,
   existing registry inventory test, gate workflow inventory/matrix, and the
   owner-authorized planning amendment. It contains no retrieval implementation or
   harness-dependent meta-test.
2. Harness commit subject/content boundary: `feat(memory): enforce mem-sec cards`.
   It contains the required public configured-agent open signature and call-site
   updates, store-open/write refusal, pass-local retrieval partition assertions,
   six-card Rust harness, harness-dependent meta-test, and this summary. Its exact SHA
   is the PR head and is intentionally not self-recorded here.

Both commits are pushed together. Fixture and label bytes are immutable in commit 2.

## Failing-first evidence

Before the Rust harness existed, the WP-3 CLI returned nonzero and the gate script
emitted exactly:

```text
FAIL MS-01 security
FAIL MS-02 relation
FAIL MS-03 security
FAIL MS-04 security
FAIL MS-05 security
FAIL MS-06 security
gate: 0/6
```

Direct failing-first exit: `2`. Missing subject never skipped.

## Focused evidence before commit 2

- `node --test gates/tests/gates-card-schema.test.cjs`: 12/12 passed.
- `cargo test -p nano-verify -- --test-threads=1`: all unit/integration/doc targets passed.
- `cargo test -p nano-memory --test mem_sec_cards -- --test-threads=1`: 7/7 passed;
  green summary `gate: 6/6`.
- `node --test gates/tests/gates-mem-sec.test.cjs`: 3/3 passed.
- `cargo test -p nano-memory -- --test-threads=1`: 23/23 passed across resolver,
  corrective regression, durability, mem-sec, recall, and mediation targets.
- `cargo clippy -p nano-memory --all-targets -- -D warnings`: passed.
- `gate.yml` local four-inventory/six-OS assertion: passed.

## Mutant-caught evidence

Detached candidate `17a1b5e52edb3a3f5ac2777b2a3954e24a4a7c48` was created without moving the
feature branch. Each committed zero-context mutant patch was applied alone, its bound
`mem_sec_N` test was run, and the patch was reversed. Runtime failures—not compile
failures—caught every mutant, with a clean detached worktree afterward:

| Check | Caught |
|---|---:|
| MS-01 | 5/5 |
| MS-02 | 5/5 |
| MS-03 | 5/5 |
| MS-04 | 5/5 |
| MS-05 | 5/5 |
| MS-06 | 5/5 |
| **Total** | **30/30** |

No mutant survived. The supplementary `memory-retrieval-recall-v1` seal remained
`5555558a1def7f320ab73949863335fb6dd9d13c2fe99c9117a255c7c1cef6a3`.

## Exact-head evidence location

Per the non-self-reference rule, the exact commit-2 `just gate-all`, WP-3
`verify --gate mem-sec --run-only`, PR review, and seven-leg CI results are recorded in
the external PR receipt/body against the immutable PR-head SHA. Commit 2 is never
amended to describe its own result.
