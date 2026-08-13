# Codex assignment: P2b image-bearing tool results lane (Wayland Nano RC2)

## Mission

Build P2b: image-bearing tool results (`view_image` + the type-widening) for Wayland Nano. The design contract is LOCKED at D:/Development/waylandnano/shared/reviews/panel-tui/P2b-image-toolresult-design.md (832 lines, REVISED-round-4 LOCKED). Build to it exactly — including the inline `[r2 ...]`/`[r3 ...]` markers (they record the audit findings that shaped it).

## Read first, in order

1. `D:/Development/waylandnano/wayland-nano/AGENTS.md` — house rules.
2. The P2b note (your contract) — every section; §3.1 (producer confinement), §3.2/§3.3 (canonical builder + journal/replay), §3.4 (per-surface codec fan-out + rung-3 refusal on completions/responses for RC2), §3.6 (gating + the image_influenced extension), §3.7 (view_image + authorization classifier + crops), §5 (security), §7 (tests), §8 (proof legs).
3. The certified P2a note's settled types are already BUILT in your base — use them, don't redesign: `TurnBlock::Image{reference, data}`, `ContentBlock::Image{mime,data}`, the attachment store, the vision catalog, the 7 vision error kinds.

## Work location + base

Worktree: `D:/Development/waylandnano/wayland-nano/.tmp-wt-p2b` (branch `feat/p2b-image-results`). Base is `scratch/p2a-integ-check` (e55a125) — P2a lanes A+B merged and gate-green. Lane A's loader (`nano_tools::image::load_image` etc.), `AttachmentStore`, `VisionCatalog`, the vision error kinds, `TurnInput`/`run_turn_streaming_with_context_blocks`, and the compaction `image_influenced` plumbing all EXIST in your base — consume them.

## What you build (the note is authoritative)

- **`nano-model/src/image_result.rs`** (new): the sealed canonical builder `build_image_tool_result(...) -> Result<(ImageToolResultParts, ImageProvenance), ImageError>` deriving projection + images + refs + digest from ONE validated ordered sequence; `ImageProvenance` pub type, private fields, NO public or crate-visible constructors — minting functions are module-private plain `fn`s; variants `Live` (builder only) and `ReplayVerified{digest}` (replay helper only); consumed exactly once (move, no Clone, #[must_use]).
- **Journal**: `Op::ToolResult` gains serde-defaulted `image_refs: Vec<ImageRef>` (digests only); `ContentBlock::ToolResult` gains `images: Vec<ImageData>` in memory. Serde round-trip + both-direction compat tests.
- **`history_image_influence`** (new `nano-agent/src/image_influence.rs`): the ONE canonical walker — true iff any ContentBlock::Image OR any non-empty ToolResult.images OR any manifest record (missing/tampered refs influential by manifest PRESENCE). Called before every ModelRequest construction AND before protected-mutation approval; the result threads to the gate via the constructor-injected shared cell (`with_image_influence(Arc<AtomicBool>)` — the C6 shared-counter pattern, already built in the base for AcpApproval; extend per the note to ExecApproval/ApproveAll/TaskApproval: unpromptable gates DENY protected mutations with a named denial_reason()).
- **The acceptance seam** (turn.rs — between `execute_cancellable` (~:1129) and `Op::ToolResult` emission (~:1137-1149), before op emission AND message insertion (~:1156-1160)): the provenance token is consumed exactly once there; rejection ⇒ journaled typed failed result (ImageInvalid). Verify the real line numbers in YOUR base (P2a shifted them).
- **Replay** (`messages_from_envelopes_rehydrating` in acp_mode.rs): the fold builds `call_names: HashMap<call_id, tool_name>` on every ToolCall (calls precede results in the append-only journal); the ToolResult arm resolves names from it (HashMap::entry + duplicate sentinel — never plain overwriting); missing/duplicate call_id ⇒ the deterministic unavailable label `[Image #N from tool <unavailable: unpaired call> — …]` + operator log, pixels still digest-verified; label lines re-derived deterministically so live == replay.
- **`view_image`** (nano-tools): runs Lane A's loader verbatim on a policy-gated read; `classify_image_read_target(canonical, policy, cwd) -> ImageReadAuth::{AutoApproved, HumanApprovalRequired, Denied}` (denials first and unchanged; workspace-approved roots auto; external sanctioned roots prompt); the injected `ImageReadApprover` trait (nano-tools) taken at handler construction — `AcpImageReadApprover` (nano-cli, bridges to the session/request_permission machinery naming the canonical target) + `DenyImageReadApprover` (headless/exec, always Denied); canonicalize → classify → act; revalidate the same canonical target immediately before open (swap ⇒ typed denial, zero bytes read). Region crops: integer pixels on the orientation-normalized raster, `x+width<=W`/`y+height<=H` checked, crop before final encode, all limits reapplied; normalized W×H surfaced in the projection label.
- **Codecs**: Anthropic arm = native image blocks in the tool_result array (fixture-pinned); completions/responses arms REFUSE image-bearing results at rung 3 for RC2 (zero-egress fixtures) — the aggregated post-turn message is deferred.

## Boundaries (hard)

- Do NOT touch: crates/nano-mcp/** (P3 lanes), the OAuth module, PTY/rules/repomap/review/session-browser lanes (other workers), packaging/** (Codex3's).
- Deviations from the note: none silently. Unbuildable ⇒ STOP and report.

## Tests (note §7)

Builder round-trip/mismatch/ordering; serde both directions; walker matrix (incl. blob-deleted-but-influential); clamp before+after compaction + kill-resume; acceptance-seam rejection (no op, no message); replay index (paired/duplicate/missing) + live-vs-replay label byte-equality; classifier battery (canonical, junction, symlink-swap, policy-denied, headless-deny); crop math; per-surface fixtures (Anthropic native + completions/responses zero-egress); the canary breadth sweep (journal/logs/errors/ACP frames/telemetry/summaries/retry bodies/3 codecs × raw/digest/base64/data-URL/substrings).

## Gates (all in the worktree)

`cargo fmt --all --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace` — all green on your base. Never weaken a test.

## Deliverable

Commits on `feat/p2b-image-results` (clear messages, NO push). Final report: files created/modified, gate numbers, exact seam signatures, deviations (expected zero).
