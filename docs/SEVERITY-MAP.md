# Severity map — open follow-ups (v2 — adjudicated per SEVERITY-SIGNOFF-2026-08-14 (Codex lens + integrator); owner signature pending)

Date: 2026-08-14 (v2). Source: `docs/FOLLOWUPS.md` (full register), verified
against git log and code. v2 is adjudicated per
`../shared/reviews/stable-wave/SEVERITY-SIGNOFF-2026-08-14.md` (Codex lens +
integrator); every v2 transition cites its evidence inline.

Severity rule (owner-locked):

- **SEV-1** — safety/security/containment/data-integrity break or spend leak.
- **SEV-2** — an advertised capability does not work on a production path, or a
  correctness defect on a shipped surface. ("Advertised capability not working
  on a production path" is sev-2 minimum.)
- **SEV-3** — everything else (fidelity, cosmetics, test robustness, doc
  reconciliation, dead code, Desktop-lane items that degrade safely).

Stable gate: **zero open sev-1/2, every status transition carries a code/test
evidence pointer.**

## SEV-1

| ID | Severity | Justification |
|---|---|---|
| F-1 | SEV-1 | No engine-side tool-result ceiling: MCP tool results (shipped) flow verbatim and uncapped into billable model-history context (`mcp.rs:1397` + `turn.rs:1637`) — an unbounded-spend / context-corruption hole on a production path. (v2 rationale correction per the sign-off: ACP frames carry digests; the leak is model-history context.) |
| Unix stdio MCP functional regression | SEV-1 | unix stdio MCP children die contained (CI: child stdout EOF); hotfix lane `fix/s5-unix-stdio` in flight. Containment itself shipped (4dc3789, merged af62482) — this row supersedes the v1 containment-gap row: containment present, function broken; CI red on unix legs until merged. |

## SEV-2

| ID | Severity | Justification |
|---|---|---|
| F-8 (data-integrity half) | SEV-2 | Cross-process cron double-fire window (cron idempotency check outside the SessionGuard, F-C11-1) plus continue-after-failed-journal-append (goal driver swallows journal failures `let _ =`; exec mid-turn journal append failure continues with a journal hole, F-C11-4/5) — journal/data-integrity defects on shipped surfaces. Split per sign-off; fix lane `fix/w2-cron` in flight. |
| F-14 | ~~SEV-2~~ FIXED (wave-2) | Promoted per sign-off: unbounded provider error-body read on a shipped route — `read_error_body` (flux_common.rs:40-42) reads `response.text()` uncapped and `classify_status` carries the provider's `error.message` whole into `ModelError::Server.message`. Transient in-process only (never reaches wire/journal/UI), but unbounded on a production path. |
| F-17 | SEV-2 | Promoted per sign-off: 46.8s worst-case cancel latency (cancel flag checked only at step boundaries) is a proven advertised-cancel defect, distinct from F-7. The id-carrying `session/cancel` half stays spec-conformant (documentation item, SEV-3 band). |
| F-18 | SEV-2 | Provider-side 404 (retired model) surfaces as `model_server_4xx{status:404}`, not `model_not_found` — fallback logic keying off the KIND (incl. P5 auto-routing) misclassifies a shipped typed-error case. |
| F-19 | SEV-2 | One `:`-bearing model id (live OpenRouter `/models`) bricks the whole provider payload fail-closed; with no Flux key the host exits at startup — availability break on a production path from a single bad entry. |
| F-27 (item 6) | SEV-2 | Turn-cancel of an in-flight MCP call is not end-to-end (registry mutex held across blocking `call_tool_mutable`; `execute_cancellable` delegates to plain `execute`) — advertised cancel partially inert; F-27's other items are LOW (see SEV-3 row). |
| F-P3-5 | SEV-2 | Promoted per sign-off: ToolSearch hydration is invisible to the current turn's tool set — every model request clones the turn-construction tool list (turn.rs:914, set once at turn.rs:242); definitions rebuild per PROMPT only (acp_mode.rs:2389-2417). Individually closeable. |
| F-P3-6 | SEV-2 | Promoted per sign-off: graceful-shutdown cancels can miss the wire — `Connection::shutdown` sets closing before enqueueing `notifications/cancelled`; the writer's closing-drain exits after one 25ms quiet tick, so late-enqueued cancels die in the lane (dispatcher.rs:1137-1152,1460-1478; F-27 item 4). Individually closeable. |
| F-P3-8 | SEV-2 | Promoted per sign-off: >64 hydrated names permanently bricks compaction — `hydration_carry_at` (nano-session/src/coordinator.rs:183-190) validates the carry against the per-op 64-name cap and aborts; every later compaction hits the same wall (fail-safe but unrecoverable). Individually closeable. |
| F-P3-11 | SEV-2 | Promoted per sign-off: OAuth loopback listener retains port+thread 180s after early failure — flow.rs:~247 binds before DCR; a `register_client` failure drops the binding but the accept loop holds the port until the 180s expiry. Now production-reachable since S5 wired the OAuth surface (79891fb). Individually closeable. |
| F-P3-12 | SEV-2 | Promoted per sign-off: unsanitized HTTP bodies/URLs in `McpError::Transport` (http.rs:59,70-75) — the "transport is dead code" premise died when S5 shipped HTTP MCP (af62482). **SEV-1 if a test proves secrets reach model/card/log paths.** Individually closeable. |
| F-P5-3 | SEV-2 | §6 "meter failed attempts" unwired in production (`failed_usage: None` hardcoded at the production adapter, auto_routing.rs:1039 — verified per sign-off) — designed usage-metering capability absent on the shipped path. |

## SEV-3

| ID | Severity | Justification |
|---|---|---|
| F-2 | SEV-3 | Desktop has no ACP diff-block renderer; TUI is the v1 renderer, Desktop degrades safely. Desktop lane. |
| F-3 | SEV-3 | Legacy Desktop `AcpConnection` drops typed error code/data, but the stack is proven dead code for every reachable backend. |
| F-4 | SEV-3 | Task-dir GC is manual by design (retention for debugging); no silent accumulation hazard beyond disk. |
| F-5 | SEV-3 | Desktop lacks C9 surfaces (steer/reconnect/rate-limit/new error kinds); unknown kinds normalize to no-ops (vitest-proven). Desktop lane. |
| F-7 | SEV-3 | Demoted per sign-off, repro-required: permission-parked turn silenced the host ≥15s in the live probe, but the cancel flag IS set promptly in the reader (acp_mode.rs:4780 — verified by the Codex lens); the silence may be a probe observation-window artifact. Close requires scripted-harness repro with a parked prompt + explicit cancel. |
| F-8 (scope half) | SEV-3 | F-C11-2: protocol-host has no cron wiring (acp-host only) — scope question, split per sign-off from the data-integrity half (SEV-2 above). |
| F-9 | SEV-3 | Mid-session AGENTS.md edit lands one turn late in acp_mode; prompt-tier only, no policy impact. |
| F-11 | SEV-3 | Doc deviation only (failed read-before-overwrite emits add-style diff); behavior intended and code-commented. |
| F-12 | SEV-3 | No currentMode push on tool-driven plan transitions — client mode display stale until next set_mode; design decision. |
| F-13 | SEV-3 | Desktop createSession null deref under polluted multi-prompt state; Desktop-side robustness, no Nano action. |
| F-15 | SEV-3 | Desktop typed-error presentation polish (5 items); wire data proven correct. Desktop lane. |
| F-16 | SEV-3 | Untyped tool failures render as bare "failed" on cards; refusal text reaches the model, only the card is terse. |
| F-17 (id-carrying cancel half) | SEV-3 | Wire-contract note: id-carrying session/cancel never fires — spec-conformant; documentation item for third-party client authors. |
| F-20 | SEV-3 | Provider model-id freshness is guidance for payload producers (Desktop); Nano carries no hardcoded lists. |
| F-23 | SEV-3 | task_spawn failure lacks a typed error_kind — structural vocabulary gap (needs a new serde-pinned NanoErrorKind variant). |
| F-24 | SEV-3 | Backend name does not cross the ACP wire card or journal-rebuilt context; fidelity residual after the masking fix (9849541). |
| F-25 | SEV-3 | Demoted per sign-off — proof debt: certified §12 cross-process GC race battery is single-process theater (attachment_store.rs:1109), but no cross-process failure has been demonstrated. |
| F-26 | SEV-3 | Demoted per sign-off: `WriteLease` not bound to store identity (attachment_store.rs:237) — latent lease-discipline hole, unreachable in shipped config (multi-store is not a production config today). |
| F-27 (items 1–5, 7–8) | SEV-3 | Dispatcher merge-review LOWs: undocumented deviations, poison-reason cosmetics, dead test-only surface, unwired contained spawn (flag pinned FALSE), test-coverage gaps. |
| F-28 (LOW-7) | SEV-3 | Windows battery owes unicode-name and >MAX_PATH cases; test-coverage debt only. |
| F-29 | SEV-3 | Rules LOWs: vestigial parse-disagreement check, order-blind differential suite, duplicated lock primitive. |
| F-30 | SEV-3 | OAuth LOWs: duplicate Host headers accepted (RFC 7230 wants 400); wincred torn chunk prefix (reads still fail closed). |
| F-31 | SEV-3 | PTY LOWs: cursor default documentation, missing ConPTY breakaway rejection test (transitively covered), unix cfg compile gap (fail-closed direction). |
| F-32 (LOW-5, LOW-6, LOW-8) | SEV-3 | LOW-6 demoted per sign-off: non-`view_image` image-bearing Live tool results are silently dropped (mechanism verified, turn.rs:1565) but no second producer exists. LOW-5/LOW-8: unavailable-label wording diverges from design's fixed string; ReplayVerified mint-then-drop and unused param — cosmetic. |
| F-33 | SEV-3 | P2a proof-process items: doc wording reconciliations + one inconclusive Flux re-probe (catalog stays false — honesty rule holds). |
| F-35 | SEV-3 | ToolSearch LOWs: fidelity-only kind emission (fail-closed), recorded covers_op_ids deviation (reviewer-verified safe), audit-only attribution race. |
| F-37 (LOW-2/3/4) | SEV-3 | Session-browser robustness lows: listing aborts on transient entry errors; error line to stdout. |
| F-38 | SEV-3 | bwrap probe test environment-sensitive on CI (ubuntu-24.04-arm); confirmed flake, no user-facing impact. |
| F-39 | SEV-3 | /doctor reports but does not sweep attachments — matches F-34's close contract; design §5.4 wording reconciliation. |
| F-40 | SEV-3 | /doctor WARN leaks attachment-audit wording; cosmetic string hygiene, rc unaffected. |
| F-LEG6R-1 | SEV-3 | Hostless `https:///mcp` dies `egress_denied` instead of typed parse refusal — fail-closed, zero socket; fidelity. |
| F-LEG6R-2 | SEV-3 | Host-bound environment limitation (Windows Credential Manager refuses all writes on the proof host); product behaves as designed. |
| F-P3-10 | SEV-3 | A forged hydration op with a non-canonical digest displaces the genuine gate digest (replay folds forged ops without validation; last-wins drops ALL genuinely hydrated tools for that server; live-proven leg3) — fail-closed direction, a journal-integrity availability hole; drop notice misreports as "inventory changed". Individually closeable. |
| F-P3 LOW list (by reference) | SEV-3 | P3 LOWs filed by reference in the manifest (CONSOLIDATED-VERIFICATION.md leg-1 list): slot-retirement declines/budget bypass, poison-reason race, drain retry, 1s tick child-exit poll, priority inversion, serverInfo retention, timeout clock reporting, schema_bytes measurement point, cron.rs JournalWriter::open bypass, granular mcpCapabilities, `requires` config refusal eprintln-only, late-answer drops, multi-field ask opaque ids, `StoredTokens` Debug, sub-8-char tokens, positional token endpoint, Windows logout refresh-file no-op. |
| F-P2B-2 | SEV-3 | ToolResult replay degradation arm emits no operator stderr log; notice cause split correct and test-pinned. |
| F-P2B-3 | SEV-3 | Rung-3 vision gate is once-per-turn; fail-closed and self-heals next prompt. |
| F-P4-4 | SEV-3 | `cmd /c <denied> <trailing args>` degrades to Prompt floor instead of Deny — fail-safe direction; never-Allow holds. |
| F-P4 lows (by reference) | SEV-3 | Manifest leg-1/leg-8 lows: repo_map prefix predicate, PTY cap check-then-insert race, pty_write unit divergence, review pre-checks, session/list params ignored, ASCII-only TS/JS identifiers. |
| F-P5-4 | SEV-3 | Merged live leg-1 test writes its fixture one `..` too many (outside workspace); test-path bug, prover cleaned the stray tree. |
| F-P5-5 | SEV-3 | Pin/implicit turn frames drop the response-reported model; moot while the provenance alias-echo gap stands. |

## FIXED at HEAD (verified against git log / code)

| ID | Fix evidence |
|---|---|
| F-6 | e3b58a5 (merged 1b8b583, `feat/s6-cron-path`) — `cronjob` wired on the ACP session surface, journal-first `CronCreated`/`CronDeleted`, create ALWAYS prompts (even full_auto), exec typed-denied; kill-window reconciliation + live kill-resume proof (`scripts/s6-proof/`). |
| F-10 | 405f0e2 — row-aware TUI question-modal viewport; selected option's rows always visible (unit pins + flipped bug-pin + regenerated snapshot). |
| F-21 | 9849541 — grounding query shaping, unit + loopback wire pins. |
| F-22 | 9849541 — chain propagates last backend's typed error. |
| F-28 (MEDIUM-1/3, LOW-4/5/6/8) | 02b538a; MEDIUM-2 closed a7065e0; LOW-8 (case-insensitive `.env.` guard) closed 405f0e2 with mixed-case pins. LOW-7 remains open (see SEV-3). |
| F-32 (LOW-7) | a7065e0 — referenced_blob_digests scans input_blocks manifests + ToolResult.image_refs. |
| F-34 | a7065e0 — startup sweep + /doctor report + image_refs scan (leg-4 concurrent re-proof owed). |
| F-36 | a7065e0 — oauth_grant_recorder at the acp_mode session layer. |
| F-37 (MEDIUM-1) | a7065e0 — explicit-id /resume sends session/load directly. |
| F-P2B-1 | 1b5306c — vision_backed conjunct refit; images carried on the flux openai-completions wire (base64 data-URI trailing user message), intake constraints typed-enforced; live Flux fixture. |
| F-P3-1 | 79891fb — OAuth login surface wired (merged 71ba0e7). |
| F-P3-2 | 575c8b3, merged 71ba0e7 — Windows contained spawn. |
| F-P3-3 | 575c8b3, merged 71ba0e7 — registration receipt / instance_id / SpecSource landed. |
| F-P3-4 | 575c8b3, merged 71ba0e7 — elicitation op-id / journaled answer fixed. |
| F-P3-7 | 575c8b3, merged 71ba0e7 — user-cancel maps to cancel. |
| F-P3-9 | S5 (merged af62482) — mismatch-path churn accounting on the digest-mismatch resume arm, per FOLLOWUPS.md. |
| F-P3-13 | 575c8b3, merged 71ba0e7 — backpressure battery fix. |
| F-P4-1 | 4a66436 (merged 2246092) — rule DSL wired end to end. |
| F-P4-2 | eeb51d7 (merged 2246092) — Windows owner-only ACL audit for rules.toml. |
| F-P4-3 | 9b9eb58 — single-writer ownership lock on journal open (`try_own` lifetime OS lock + typed `Busy`); live-proven session double-load closed. |
| F-P5-1 | 557a055 — terminal 5xx format rejection wired on the production path. |
| F-P5-2 | 557a055 — kill-resume budget leak closed: journaled remainder, regression coverage pinned. |
| MCP HTTP typed-refusal row | Shipped af62482 (`feat/s5-mcp-http`) — HTTP MCP transport is now live via dispatcher through egress; the v1 fail-closed typed-refusal row is obsolete and deleted. |
| Unix stdio MCP containment | 4dc3789 (merged af62482) — seatbelt/bwrap + process-group guardian landed; superseded by the open SEV-1 functional-regression row above (containment present, function broken; `fix/s5-unix-stdio` in flight). |

## Notes

- v2 transitions: fixed rows cite their commits; F-7/F-25/F-26/F-32-LOW-6
  demotions and F-14/F-17/F-P3-5/F-P3-6/F-P3-8/F-P3-11/F-P3-12 promotions are
  per `../shared/reviews/stable-wave/SEVERITY-SIGNOFF-2026-08-14.md` with the
  verifying code lines inline. Owner signature on the sign-off is pending.
- The two "Pending FOLLOWUPS entries — to ride the next merge commit" blocks in
  FOLLOWUPS.md overlap; this map deduplicates by F-id.
- F-8 is split per the sign-off: the data-integrity half (SEV-2) tracks the
  cross-process cron double-fire window and continue-after-failed-journal-append
  on the `fix/w2-cron` lane; the scope half (F-C11-2 protocol-host cron wiring)
  is SEV-3.
- F-P3-5..F-P3-12 are individually closeable rows: 5/6/8/11/12 promoted SEV-2,
  7 fixed (575c8b3), 9 fixed (S5, af62482), 10 remains SEV-3.
- Unnumbered pending merge-review lows (P5 auto_routing comment-vs-journal
  drift; exec CandidateInputs stubs — INFO; P3 fixround `is_https_url`/grant-
  journal wording; P3 leg-6 items now numbered F-LEG6R-1/2) are all SEV-3 band.
