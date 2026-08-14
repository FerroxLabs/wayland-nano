# Severity map — open follow-ups (proposal, owner-signed at adoption)

Date: 2026-08-14. Source: `docs/FOLLOWUPS.md` (full register), verified against
git log and HEAD code. This is the **proposal**; the owner signs by editing in
place.

Severity rule (owner-locked):

- **SEV-1** — safety/security/containment/data-integrity break or spend leak.
- **SEV-2** — an advertised capability does not work on a production path, or a
  correctness defect on a shipped surface. ("Advertised capability not working
  on a production path" is sev-2 minimum.)
- **SEV-3** — everything else (fidelity, cosmetics, test robustness, doc
  reconciliation, dead code, Desktop-lane items that degrade safely).

## SEV-1

| ID | Severity | Justification |
|---|---|---|
| F-1 | SEV-1 | No engine-side tool-result ceiling: MCP tool results (shipped) flow verbatim and uncapped into billable model history and ACP `rawOutput` — an unbounded-spend / context-corruption hole on a production path. |
| F-28 (LOW-8) | SEV-1 | `sensitive_path.rs:25` `.env.` prefix check is case-sensitive, so `.ENV.PRODUCTION` slips the credential-store guard on case-insensitive filesystems — a security-guard bypass on the primary platform (Windows). |
| F-P4-3 | SEV-1 | Live-proven session double-load: a second host `session/load` succeeded while host 1 held the session mid-turn — two hosts can append-mark the same journal, breaking the append-only data-integrity invariant. |
| F-P5-2 | SEV-1 | Chained kill-resume budget leak: `journal_snapshot` hardcodes `attempt_budget` (auto_routing.rs:873), so a second kill replays stale budget — 4 physical attempts across one logical routed turn; a spend leak past the configured bound. |
| Unix stdio MCP containment gap | SEV-1 | `crates/nano-mcp/src/stdio.rs:15-22`: unix stdio MCP children spawn via plain `std::process::Command` — no direct-descendant kill, no host-death reaping. Containment break on a reachable registration path (`mcp.rs` `register()` gates only HTTP); Windows is contained (F-P3-2 fix); capability flag pinned FALSE until §13 leg-1b — the residual is the unix gap. |

## SEV-2

| ID | Severity | Justification |
|---|---|---|
| F-6 | SEV-2 | Cron job creation has no production path: `cronjob` absent from the interactive ACP tool list and exec auto-denies `create` — the designed creation surface (design §5.5 prompt under full_auto) is unreachable; only the fire path is proven. Owner ruling needed. |
| F-7 | SEV-2 | Permission-parked turn silences the host ≥15s (live probe): fork/prompt/cancel unobserved while parked — cancel, an advertised capability, potentially wedged on a production path; unconfirmed vs observation-window artifact, repro owed. |
| F-8 | SEV-2 | C11 hardening set: cron idempotency check outside the SessionGuard (cross-process double-fire window) plus journal holes on goal-driver/exec failure paths — journal-integrity defects on shipped surfaces. |
| F-18 | SEV-2 | Provider-side 404 (retired model) surfaces as `model_server_4xx{status:404}`, not `model_not_found` — fallback logic keying off the KIND (incl. P5 auto-routing) misclassifies a shipped typed-error case. |
| F-19 | SEV-2 | One `:`-bearing model id (live OpenRouter `/models`) bricks the whole provider payload fail-closed; with no Flux key the host exits at startup — availability break on a production path from a single bad entry. |
| F-25 | SEV-2 | Certified §12 cross-process GC race battery is single-process theater (attachment_store.rs:1109) — the blob-store GC's cross-process safety claim is unproven; worst case is deletion of an in-flight blob (data integrity). |
| F-26 | SEV-2 | `WriteLease` not bound to store identity (attachment_store.rs:237): a lease from store A can authorize `put` into store B during B's GC — latent integrity hole in the lease discipline (multi-store not a production config today). |
| F-27 (item 6) | SEV-2 | Turn-cancel of an in-flight MCP call is not end-to-end (registry mutex held across blocking `call_tool_mutable`; `execute_cancellable` delegates to plain `execute`) — advertised cancel partially inert; F-27's other items are LOW (see SEV-3 row). |
| F-32 (LOW-6) | SEV-2 | Non-`view_image` image-bearing Live tool results are silently dropped (turn.rs:1347) — silent data loss to the model on a shipped surface; F-32's other items are LOW. |
| F-P2B-1 | SEV-2 | Live-proven both wires: `view_image` unreachable in every shipped config (vision_backed conjuncts mutually unsatisfiable) — the vision capability ships inert. Deferred to the post-P5-merge wave; still open at HEAD. |
| F-P5-1 | SEV-2 | 500-with-format-body cascades: §8.1's terminal conflict class holds only in a pure-function unit; production never populates the `body` signal (auto_routing.rs:566-620, flux_common.rs:63-86) — documented routing behavior does not hold on the production wire. |
| F-P5-3 | SEV-2 | §6 "meter failed attempts" unwired in production (`failed_usage: None` hardcoded, auto_routing.rs:966-972) — designed usage-metering capability absent on the shipped path. |

## SEV-3

| ID | Severity | Justification |
|---|---|---|
| F-2 | SEV-3 | Desktop has no ACP diff-block renderer; TUI is the v1 renderer, Desktop degrades safely. Desktop lane. |
| F-3 | SEV-3 | Legacy Desktop `AcpConnection` drops typed error code/data, but the stack is proven dead code for every reachable backend. |
| F-4 | SEV-3 | Task-dir GC is manual by design (retention for debugging); no silent accumulation hazard beyond disk. |
| F-5 | SEV-3 | Desktop lacks C9 surfaces (steer/reconnect/rate-limit/new error kinds); unknown kinds normalize to no-ops (vitest-proven). Desktop lane. |
| F-9 | SEV-3 | Mid-session AGENTS.md edit lands one turn late in acp_mode; prompt-tier only, no policy impact. |
| F-10 | SEV-3 | TUI question modal clips the 4th option; still operable blind, Esc safe; 2-option flows unaffected. |
| F-11 | SEV-3 | Doc deviation only (failed read-before-overwrite emits add-style diff); behavior intended and code-commented. |
| F-12 | SEV-3 | No currentMode push on tool-driven plan transitions — client mode display stale until next set_mode; design decision. |
| F-13 | SEV-3 | Desktop createSession null deref under polluted multi-prompt state; Desktop-side robustness, no Nano action. |
| F-14 | SEV-3 | Provider error bodies read unbounded; transient in-process memory only — never reaches wire/journal/UI (statically bounded, canary-proven). |
| F-15 | SEV-3 | Desktop typed-error presentation polish (5 items); wire data proven correct. Desktop lane. |
| F-16 | SEV-3 | Untyped tool failures render as bare "failed" on cards; refusal text reaches the model, only the card is terse. |
| F-17 | SEV-3 | Wire-contract note: id-carrying session/cancel never fires — spec-conformant; documentation item. |
| F-20 | SEV-3 | Provider model-id freshness is guidance for payload producers (Desktop); Nano carries no hardcoded lists. |
| F-23 | SEV-3 | task_spawn failure lacks a typed error_kind — structural vocabulary gap (needs a new serde-pinned NanoErrorKind variant). |
| F-24 | SEV-3 | Backend name does not cross the ACP wire card or journal-rebuilt context; fidelity residual after the masking fix (9849541). |
| F-27 (items 1–5, 7–8) | SEV-3 | Dispatcher merge-review LOWs: undocumented deviations, poison-reason cosmetics, dead test-only surface, unwired contained spawn (flag pinned FALSE), test-coverage gaps. |
| F-28 (LOW-7) | SEV-3 | Windows battery owes unicode-name and >MAX_PATH cases; test-coverage debt only. |
| F-29 | SEV-3 | Rules LOWs: vestigial parse-disagreement check, order-blind differential suite, duplicated lock primitive. |
| F-30 | SEV-3 | OAuth LOWs: duplicate Host headers accepted (RFC 7230 wants 400); wincred torn chunk prefix (reads still fail closed). |
| F-31 | SEV-3 | PTY LOWs: cursor default documentation, missing ConPTY breakaway rejection test (transitively covered), unix cfg compile gap (fail-closed direction). |
| F-32 (LOW-5, LOW-8) | SEV-3 | Unavailable-label wording diverges from design's fixed string; ReplayVerified mint-then-drop and unused param — cosmetic. |
| F-33 | SEV-3 | P2a proof-process items: doc wording reconciliations + one inconclusive Flux re-probe (catalog stays false — honesty rule holds). |
| F-35 | SEV-3 | ToolSearch LOWs: fidelity-only kind emission (fail-closed), recorded covers_op_ids deviation (reviewer-verified safe), audit-only attribution race. |
| F-37 (LOW-2/3/4) | SEV-3 | Session-browser robustness lows: listing aborts on transient entry errors; error line to stdout. |
| F-38 | SEV-3 | bwrap probe test environment-sensitive on CI (ubuntu-24.04-arm); confirmed flake, no user-facing impact. |
| F-39 | SEV-3 | /doctor reports but does not sweep attachments — matches F-34's close contract; design §5.4 wording reconciliation. |
| F-40 | SEV-3 | /doctor WARN leaks attachment-audit wording; cosmetic string hygiene, rc unaffected. |
| F-LEG6R-1 | SEV-3 | Hostless `https:///mcp` dies `egress_denied` instead of typed parse refusal — fail-closed, zero socket; fidelity. |
| F-LEG6R-2 | SEV-3 | Host-bound environment limitation (Windows Credential Manager refuses all writes on the proof host); product behaves as designed. |
| F-P3-5..F-P3-12 + LOW list | SEV-3 | P3 MEDIUM/LOW filed by reference in the manifest (static tool definitions vs §3.2, shutdown cancel misses, compaction/churn/DCR-listener items, http.rs error leakage); none alleges a broken production path. |
| F-P2B-2 | SEV-3 | ToolResult replay degradation arm emits no operator stderr log; notice cause split correct and test-pinned. |
| F-P2B-3 | SEV-3 | Rung-3 vision gate is once-per-turn; fail-closed and self-heals next prompt. |
| F-P4-4 | SEV-3 | `cmd /c <denied> <trailing args>` degrades to Prompt floor instead of Deny — fail-safe direction; never-Allow holds. |
| F-P4 lows (by reference) | SEV-3 | Manifest leg-1/leg-8 lows: repo_map prefix predicate, PTY cap check-then-insert race, pty_write unit divergence, review pre-checks, session/list params ignored, ASCII-only TS/JS identifiers. |
| F-P5-4 | SEV-3 | Merged live leg-1 test writes its fixture one `..` too many (outside workspace); test-path bug, prover cleaned the stray tree. |
| F-P5-5 | SEV-3 | Pin/implicit turn frames drop the response-reported model; moot while the provenance alias-echo gap stands. |
| MCP HTTP typed-refusal | SEV-3 | `crates/nano-agent/src/mcp.rs:442-445`: HTTP specs register as a typed, loud refusal (§6.1 transport unwired) — fail-closed and honestly advertised (`mcpCapabilities.http:false` on the ACP initialize). Capability gap, not a broken advertised surface. |

## FIXED at HEAD (verified against git log / code)

| ID | Fix evidence |
|---|---|
| F-21 | 9849541 — grounding query shaping, unit + loopback wire pins. |
| F-22 | 9849541 — chain propagates last backend's typed error. |
| F-34 | a7065e0 — startup sweep + /doctor report + image_refs scan (leg-4 concurrent re-proof owed). |
| F-36 | a7065e0 — oauth_grant_recorder at the acp_mode session layer. |
| F-28 (MEDIUM-1/3, LOW-4/5/6) | 02b538a; MEDIUM-2 closed a7065e0. LOW-7/LOW-8 remain open (see above). |
| F-32 (LOW-7) | a7065e0 — referenced_blob_digests scans input_blocks manifests + ToolResult.image_refs. |
| F-37 (MEDIUM-1) | a7065e0 — explicit-id /resume sends session/load directly. |
| F-P3-1 | 79891fb — OAuth login surface wired (merged 71ba0e7). |
| F-P3-2 | 575c8b3, merged 71ba0e7 — Windows contained spawn (unix residual tracked above). |
| F-P3-3 | 575c8b3, merged 71ba0e7 — registration receipt / instance_id / SpecSource landed. |
| F-P3-4 | 575c8b3, merged 71ba0e7 — elicitation op-id / journaled answer fixed. |
| F-P3-7 | 575c8b3, merged 71ba0e7 — user-cancel maps to cancel. |
| F-P3-13 | 575c8b3, merged 71ba0e7 — backpressure battery fix. |
| F-P4-1 | 4a66436 (merged 2246092) — rule DSL wired end to end. |
| F-P4-2 | eeb51d7 (merged 2246092) — Windows owner-only ACL audit for rules.toml. |

## Notes

- The two "Pending FOLLOWUPS entries — to ride the next merge commit" blocks in
  FOLLOWUPS.md overlap; this map deduplicates by F-id.
- Unnumbered pending merge-review lows (P5 auto_routing comment-vs-journal
  drift — subsumed by F-P5-2's fix surface; exec CandidateInputs stubs — INFO;
  P3 fixround `is_https_url`/grant-journal wording; P3 leg-6 items now numbered
  F-LEG6R-1/2) are all SEV-3 band.
- F-P2B-1 stays open at HEAD: post-P5-merge commits (687512c..HEAD) contain no
  vision-gating change; `FluxDriver::anthropic_compat` remains uncalled.
