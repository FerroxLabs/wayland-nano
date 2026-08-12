//! Streaming-safe terminal-sequence sanitizer (normative — design doc §5/D2).
//!
//! EVERY string the TUI renders (model output, tool rawInput/rawOutput,
//! doctor text, replayed transcripts) passes through this stateful
//! incremental parser. It neutralizes every escape family — CSI, OSC, DCS,
//! APC, PM, SOS, plus ESC-initiated Fe/Fs/nF sequences — and all C0/C1
//! control characters except `\n` and `\t`.
//!
//! "Streaming-safe": the parser state carries across chunk boundaries, so an
//! escape split across two streamed frames (`ESC` + `[38;5` in one chunk,
//! `;9m` in the next) cannot evade the filter or spoof UI chrome. Sequence
//! bytes are dropped as they are consumed — never buffered — so an
//! unterminated sequence at end-of-stream is dropped, never forwarded
//! ([`Sanitizer::finish`] simply abandons the pending state).
//!
//! The policy is fail-closed drop-everything: output is produced ONLY in the
//! Ground state, and only for characters that are not controls and do not
//! begin or continue any escape sequence. Nothing is ever "escaped visibly"
//! or passed through for the terminal to interpret.

/// Parser states. All non-Ground states mean "inside a sequence — drop".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    /// Normal text. Only here do we emit.
    #[default]
    Ground,
    /// Saw ESC; deciding the sequence family from the next char.
    Esc,
    /// ESC followed by nF intermediates (0x20-0x2F), e.g. `ESC ( 0`
    /// (charset designation — changes glyph rendering). Waiting for the
    /// final byte (0x30-0x7E).
    EscNf,
    /// Inside CSI (`ESC [` or C1 0x9B) until a final byte (0x40-0x7E).
    Csi,
    /// Inside OSC (`ESC ]` or C1 0x9D) until BEL or ST.
    Osc,
    /// Inside DCS/SOS/PM/APC until ST. Only ST (or CAN/SUB abort) ends
    /// these; the `bel` variant is handled by [`State::Osc`].
    StringSeq,
}

const ESC: char = '\u{1b}';
const BEL: char = '\u{07}';
const CAN: char = '\u{18}';
const SUB: char = '\u{1a}';
const DEL: char = '\u{7f}';
/// C1 ST (String Terminator), U+009C.
const C1_ST: char = '\u{9c}';

/// C1 single-char sequence introducers (8-bit forms). ECMA-48:
/// 0x90 DCS, 0x98 SOS, 0x9B CSI, 0x9D OSC, 0x9E PM, 0x9F APC.
fn c1_introducer(c: char) -> Option<State> {
    match c {
        '\u{90}' => Some(State::StringSeq), // DCS
        '\u{98}' => Some(State::StringSeq), // SOS
        '\u{9b}' => Some(State::Csi),       // CSI
        '\u{9d}' => Some(State::Osc),       // OSC
        '\u{9e}' => Some(State::StringSeq), // PM
        '\u{9f}' => Some(State::StringSeq), // APC
        _ => None,
    }
}

fn is_c0(c: char) -> bool {
    ('\u{00}'..='\u{1f}').contains(&c)
}

fn is_c1(c: char) -> bool {
    ('\u{80}'..='\u{9f}').contains(&c)
}

/// Stateful incremental sanitizer. One instance per rendered stream (e.g.
/// one per active transcript cell); state persists across [`push`](Self::push)
/// calls and across ACP chunk boundaries.
#[derive(Debug, Default)]
pub struct Sanitizer {
    state: State,
}

impl Sanitizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk; returns the sanitized text safe to render.
    ///
    /// Escape-sequence bytes are dropped the moment they are consumed, so a
    /// sequence split across chunks leaves no residue in the output and the
    /// carried state cannot be evaded by frame boundaries.
    pub fn push(&mut self, chunk: &str) -> String {
        let mut out = String::with_capacity(chunk.len());
        for c in chunk.chars() {
            self.step(c, &mut out);
        }
        out
    }

    /// Signal end-of-stream. Any unterminated sequence pending in the parser
    /// is dropped (never forwarded); the parser returns to Ground.
    pub fn finish(&mut self) {
        self.state = State::Ground;
    }

    fn step(&mut self, c: char, out: &mut String) {
        match self.state {
            State::Ground => match c {
                // The only controls allowed to render.
                '\n' | '\t' => out.push(c),
                ESC => self.state = State::Esc,
                CAN | SUB | DEL => {}
                _ if is_c0(c) => {}
                _ if c == C1_ST || is_c1(c) => {
                    if let Some(next) = c1_introducer(c) {
                        self.state = next;
                    }
                    // Other C1 controls (IND, NEL, SS2, ...) drop in place.
                    // NEL is NOT converted to a newline — fail-closed.
                }
                _ => out.push(c),
            },
            State::Esc => match c {
                '[' => self.state = State::Csi,
                ']' => self.state = State::Osc,
                // DCS / SOS / PM / APC — string sequences, ST-terminated.
                'P' | 'X' | '^' | '_' => self.state = State::StringSeq,
                // nF sequence: intermediates then a final byte (charset
                // designation, DECSC, ...). Consume until the final.
                '\u{20}'..='\u{2f}' => self.state = State::EscNf,
                ESC => {} // ESC ESC — the second restarts the sequence.
                CAN | SUB => self.state = State::Ground,
                // Fe/Fs/Fp single-char sequence (ESC c, ESC 7, ESC =, ST `\`
                // after a string body, ...): complete — drop and done.
                '\u{30}'..='\u{7e}' => self.state = State::Ground,
                // Anything else (C0, C1, DEL, ≥0x80): abort the sequence.
                _ => self.state = State::Ground,
            },
            State::EscNf => match c {
                '\u{30}'..='\u{7e}' => self.state = State::Ground,
                ESC => self.state = State::Esc,
                CAN | SUB => self.state = State::Ground,
                _ => {} // intermediates, C0/C1 garbage: drop, stay.
            },
            State::Csi => match c {
                // Final byte ends the sequence.
                '\u{40}'..='\u{7e}' => self.state = State::Ground,
                ESC => self.state = State::Esc,
                CAN | SUB => self.state = State::Ground,
                _ => {} // params/intermediates/C0/C1/≥0x7F: drop, stay.
            },
            State::Osc => match c {
                BEL | C1_ST => self.state = State::Ground,
                // ESC \ terminates via the Esc state ('\' completes an Fe
                // sequence there); ESC anything-else aborts and restarts.
                ESC => self.state = State::Esc,
                CAN | SUB => self.state = State::Ground,
                _ => {}
            },
            State::StringSeq => match c {
                C1_ST => self.state = State::Ground,
                ESC => self.state = State::Esc,
                CAN | SUB => self.state = State::Ground,
                _ => {}
            },
        }
    }
}

/// One-shot convenience for strings that are complete (tool rawInput, file
/// contents, doctor output): sanitize and drop any trailing unterminated
/// sequence.
pub fn sanitize(text: &str) -> String {
    let mut s = Sanitizer::new();
    let out = s.push(text);
    s.finish();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exhaustive split-point property: sanitizing a payload in one push
    /// MUST equal sanitizing it across ANY split into two chunks. This is
    /// the streaming-safety invariant (D2) — an escape split across streamed
    /// frames cannot evade the filter.
    fn assert_split_invariant(payload: &str) {
        let whole = sanitize(payload);
        let boundaries: Vec<usize> = payload
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(payload.len()))
            .collect();
        for cut in boundaries {
            let mut s = Sanitizer::new();
            let mut streamed = s.push(&payload[..cut]);
            streamed.push_str(&s.push(&payload[cut..]));
            s.finish();
            assert_eq!(
                whole, streamed,
                "split at byte {cut} diverged for payload {payload:?}"
            );
        }
    }

    /// A payload sanitized in one pass must never contain ESC, C0 (except
    /// \n \t), C1, or DEL — the output contract the renderer relies on.
    fn assert_clean(out: &str) {
        for c in out.chars() {
            assert!(
                !is_c0(c) && !is_c1(c) && c != DEL || c == '\n' || c == '\t',
                "unclean char {c:?} in {out:?}"
            );
        }
    }

    #[test]
    fn plain_text_passes_through() {
        let text = "hello world\nline two\ttabbed — ünïcødé ✓\n";
        assert_eq!(sanitize(text), text);
    }

    #[test]
    fn csi_families_dropped() {
        assert_eq!(sanitize("a\x1b[31mred\x1b[0mb"), "aredb");
        assert_eq!(sanitize("x\x1b[2Jy"), "xy");
        assert_eq!(sanitize("x\x1b[1;2Hy"), "xy");
        assert_eq!(sanitize("x\x1b[?25ly"), "xy"); // private-mode prefix
        assert_eq!(sanitize("x\x1b[38;5;9my"), "xy");
        assert_eq!(sanitize("x\x1b[s\x1b[uy"), "xy"); // save/restore cursor
    }

    #[test]
    fn osc_dropped_both_terminators() {
        // BEL-terminated (window title spoof attempt).
        assert_eq!(sanitize("a\x1b]0;fake title\x07b"), "ab");
        // ST-terminated.
        assert_eq!(sanitize("a\x1b]8;;http://evil\x1b\\linkb"), "alinkb");
        // C1-ST-terminated.
        assert_eq!(sanitize("a\x1b]0;t\u{9c}b"), "ab");
    }

    #[test]
    fn dcs_apc_pm_sos_dropped() {
        assert_eq!(sanitize("a\x1bPq$dcs payload\x1b\\b"), "ab");
        assert_eq!(sanitize("a\x1b_Gapc payload\x1b\\b"), "ab");
        assert_eq!(sanitize("a\x1b^pm payload\x1b\\b"), "ab");
        assert_eq!(sanitize("a\x1bXsos payload\x1b\\b"), "ab");
        // C1-ST terminators.
        assert_eq!(sanitize("a\x1bPpayload\u{9c}b"), "ab");
        assert_eq!(sanitize("a\x1b_Gpayload\u{9c}b"), "ab");
    }

    #[test]
    fn c1_8bit_introducers_dropped() {
        assert_eq!(sanitize("a\u{9b}31mb"), "ab"); // C1 CSI
        assert_eq!(sanitize("a\u{9d}0;t\x07b"), "ab"); // C1 OSC
        assert_eq!(sanitize("a\u{90}payload\u{9c}b"), "ab"); // C1 DCS
        assert_eq!(sanitize("a\u{9f}payload\u{9c}b"), "ab"); // C1 APC
        assert_eq!(sanitize("a\u{9e}payload\u{9c}b"), "ab"); // C1 PM
        assert_eq!(sanitize("a\u{98}payload\u{9c}b"), "ab"); // C1 SOS
        // Lone C1 controls (incl. NEL) drop, never become newlines.
        assert_eq!(sanitize("a\u{85}b"), "ab");
        assert_eq!(sanitize("a\u{9c}b"), "ab"); // stray ST
    }

    #[test]
    fn c0_and_del_dropped_except_lf_tab() {
        assert_eq!(sanitize("a\x00b\x07c\x0bd\x0ce\x7ff"), "abcdef");
        assert_eq!(sanitize("a\rb"), "ab"); // CR is not allowed through
        assert_eq!(sanitize("a\nb\tc"), "a\nb\tc");
    }

    #[test]
    fn fe_and_nf_sequences_dropped() {
        assert_eq!(sanitize("a\x1bc b"), "a b"); // RIS — wait, space is text
        assert_eq!(sanitize("a\x1bcb"), "ab"); // RIS
        assert_eq!(sanitize("a\x1b7b\x1b8c"), "abc"); // DECSC/DECRC
        assert_eq!(sanitize("a\x1b(0b"), "ab"); // DEC line-drawing charset (nF)
        assert_eq!(sanitize("a\x1b%Gb"), "ab"); // charset select (nF)
        assert_eq!(sanitize("a\x1b#8b"), "ab"); // DECALN (nF)
    }

    #[test]
    fn unterminated_sequences_dropped_at_finish() {
        assert_eq!(sanitize("text\x1b[38;5"), "text");
        assert_eq!(sanitize("text\x1b]0;title"), "text");
        assert_eq!(sanitize("text\x1bPdcs"), "text");
        assert_eq!(sanitize("text\x1b"), "text");
        assert_eq!(sanitize("text\x1b("), "text");
    }

    #[test]
    fn aborts_cannot_smuggle_text_into_sequences() {
        // CAN/SUB abort: trailing bytes after abort are plain text — but the
        // ESC itself is already gone, so nothing reaches the terminal.
        assert_eq!(sanitize("a\x1b[31\x18m visible"), "am visible");
        // ESC restarts mid-sequence: the second sequence is consumed too.
        assert_eq!(sanitize("a\x1b]0;t\x1b[31mb"), "ab");
    }

    #[test]
    fn split_frame_escape_cannot_evade() {
        // The canonical D2 attack: ESC at the end of one streamed chunk,
        // the CSI body in the next.
        let mut s = Sanitizer::new();
        assert_eq!(s.push("answer: \x1b"), "answer: ");
        assert_eq!(s.push("[38;5"), "");
        assert_eq!(s.push(";9m REST"), " REST");
        s.finish();
    }

    #[test]
    fn split_st_across_chunks() {
        // OSC body ends in one chunk; its ESC \ terminator arrives split.
        let mut s = Sanitizer::new();
        assert_eq!(s.push("a\x1b]0;title\x1b"), "a");
        assert_eq!(s.push("\\b"), "b");
        s.finish();
    }

    #[test]
    fn split_invariant_over_hostile_corpus() {
        let payloads = [
            "plain text\nwith\tallowed controls",
            "\x1b[31mred\x1b[0m",
            "a\x1b[38;5;9mb",
            "\x1b]0;window title\x07rest",
            "\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\",
            "\x1bP1$r\x1b\\after",
            "\x1b_Gpayload\x1b\\after",
            "\x1b^pm\x1b\\\x1bXsos\x1b\\",
            "\x1b(0line-drawing\x1b(B",
            "\x1bc\x1b7save\x1b8",
            "\u{9b}2J\u{9d}0;t\x07\u{90}d\u{9c}",
            "mix\x1b[1;2H and \x1b]0;t\x07 and \x1bPd\x1b\\ end",
            "\x00\x01\x02\x7f\u{80}\u{85}\u{9c}clean",
            "unterminated\x1b[38;5",
            "unterminated osc\x1b]0;title",
            "\x1b\x1b\x1b[31mdouble esc",
            "can abort\x1b[31\x18text",
            "newline inside csi\x1b[31\nm after",
        ];
        for payload in payloads {
            assert_split_invariant(payload);
            assert_clean(&sanitize(payload));
        }
    }

    #[test]
    fn sanitized_output_is_idempotent() {
        let payload = "a\x1b[31mb\x1b]0;t\x07c\x1bPd\x1b\\e\nf\tg";
        let once = sanitize(payload);
        assert_eq!(sanitize(&once), once);
    }

    #[test]
    fn state_isolates_streams() {
        // Two independent Sanitizers (two transcript cells) must not share
        // state: a pending escape in one cannot eat the other's text.
        let mut a = Sanitizer::new();
        let mut b = Sanitizer::new();
        assert_eq!(a.push("left off mid-escape \x1b["), "left off mid-escape ");
        assert_eq!(b.push("untouched"), "untouched");
        assert_eq!(a.push("31mrest"), "rest");
    }

    #[test]
    fn huge_paste_bomb_is_bounded_work() {
        // 1 MiB of escape soup: linear time, no buffering blow-up.
        let bomb = "\x1b[31m".repeat(200_000);
        let out = sanitize(&bomb);
        assert_eq!(out, "");
    }
}
