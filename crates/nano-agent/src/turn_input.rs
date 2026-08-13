//! P2a §5.2.1 — `TurnInput`: the ONE authoritative host-side value every
//! image-bearing (or text-only) turn starts from. The §2.3 ACP converter
//! AND the TUI attach path both produce `TurnInput` DIRECTLY — no
//! intermediate `Vec<ContentBlock>` or bare `Vec<InputBlock>` form, no third
//! producer. From this one value, [`TurnInput::projection`] derives the
//! placeholder-bearing `TurnBegin.input` string, [`TurnInput::manifest`]
//! derives the digests-only journal manifest, and
//! [`TurnInput::content_blocks`] derives the live first user `Message` —
//! three views of one source, so the journal and the dispatched context can
//! never diverge (the §12 journal-vs-first-message oracle asserts exactly
//! this equality before dispatch).

use nano_model::types::ContentBlock;
use nano_session::op::ImageRef;
use nano_session::op::InputBlock;

/// The single value every image-bearing (or text-only) turn starts from.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnInput {
    /// Ordered blocks — order and multiplicity are preserved exactly (the
    /// §5.2 ordered manifest is the machine contract).
    pub blocks: Vec<TurnBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnBlock {
    Text {
        text: String,
    },
    Image {
        /// Durable half — serialized verbatim into `TurnBegin.input_blocks`
        /// as `InputBlock::ImageRef` (digest/mime/bytes/w/h/placeholder,
        /// digests only, §5.2).
        reference: ImageRef,
        /// Live half — the validated, re-encoded base64 pixels (no data-URI
        /// prefix) from which the request-time `ContentBlock::Image` is
        /// built at codec call time. NEVER journaled; on replay it is
        /// refilled from the blob store by `reference.digest`,
        /// digest-verified (§5.3).
        data: String,
    },
}

impl TurnInput {
    /// The legacy `&str` entry delegation: one Text block. The projection
    /// of the result is byte-identical to `s` (the §12 regression over the
    /// six legacy entry points).
    pub fn text(s: &str) -> Self {
        Self {
            blocks: vec![TurnBlock::Text {
                text: s.to_string(),
            }],
        }
    }

    /// → `TurnBegin.input`: the placeholder-bearing plain-text projection
    /// (display, `replay_frames`, old readers). Text blocks pass through
    /// verbatim; each image contributes its display placeholder. Blocks are
    /// joined with `\n`, matching the pre-P2a ACP text-part join.
    pub fn projection(&self) -> String {
        self.blocks
            .iter()
            .map(|block| match block {
                TurnBlock::Text { text } => text.as_str(),
                TurnBlock::Image { reference, .. } => reference.placeholder.as_str(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// → `TurnBegin.input_blocks`: the digests-only ordered journal
    /// manifest (NEVER bytes — the digest-only invariant, §5.2).
    pub fn manifest(&self) -> Vec<InputBlock> {
        self.blocks
            .iter()
            .map(|block| match block {
                TurnBlock::Text { text } => InputBlock::Text { text: text.clone() },
                TurnBlock::Image { reference, .. } => InputBlock::ImageRef(reference.clone()),
            })
            .collect()
    }

    /// → the live first user `Message`'s content blocks: Text →
    /// `ContentBlock::Text`, Image → `ContentBlock::Image { mime, data }`
    /// built from the block's live half (whose metadata was filled by the
    /// §4 loader host-side BEFORE the run entry).
    pub fn content_blocks(&self) -> Vec<ContentBlock> {
        self.blocks
            .iter()
            .map(|block| match block {
                TurnBlock::Text { text } => ContentBlock::Text { text: text.clone() },
                TurnBlock::Image { reference, data } => ContentBlock::Image {
                    mime: reference.mime.clone(),
                    data: data.clone(),
                },
            })
            .collect()
    }

    /// Whether any block carries an image (rung-1/rung-3 gating §6.2 and
    /// the §9.1 image-influenced session flag).
    pub fn has_images(&self) -> bool {
        self.blocks
            .iter()
            .any(|block| matches!(block, TurnBlock::Image { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_block(digest: &str, placeholder: &str) -> TurnBlock {
        TurnBlock::Image {
            reference: ImageRef {
                digest: digest.into(),
                mime: "image/png".into(),
                bytes: 100,
                width: 8,
                height: 8,
                placeholder: placeholder.into(),
            },
            data: "aGVsbG8".into(),
        }
    }

    /// §12 producer plumbing: `TurnInput::text` is behavior-identical to the
    /// legacy &str entries — projection round-trips the input byte-exactly.
    #[test]
    fn text_delegation_projects_byte_identically() {
        let input = TurnInput::text("fix the build\nplease");
        assert_eq!(input.projection(), "fix the build\nplease");
        assert_eq!(
            input.manifest(),
            vec![InputBlock::Text {
                text: "fix the build\nplease".into()
            }]
        );
        assert_eq!(
            input.content_blocks(),
            vec![ContentBlock::Text {
                text: "fix the build\nplease".into()
            }]
        );
        assert!(!input.has_images());
    }

    /// §5.2.1: three views of ONE source — projection carries the
    /// placeholder, the manifest carries the digest and NEVER the data,
    /// content blocks carry the live pixels. Order/multiplicity preserved.
    #[test]
    fn three_views_of_one_source_cannot_diverge() {
        let input = TurnInput {
            blocks: vec![
                image_block(&"aa".repeat(32), "[Image #1: /tmp/a.png]"),
                TurnBlock::Text {
                    text: "between".into(),
                },
                image_block(&"bb".repeat(32), "[Image #2: /tmp/b.png]"),
            ],
        };
        assert_eq!(
            input.projection(),
            "[Image #1: /tmp/a.png]\nbetween\n[Image #2: /tmp/b.png]"
        );
        let manifest = input.manifest();
        assert_eq!(manifest.len(), 3);
        // Digest-only: the manifest NEVER carries the live base64 half.
        let serialized = serde_json::to_string(&manifest).expect("serialize");
        assert!(!serialized.contains("aGVsbG8"));
        assert!(serialized.contains(&"aa".repeat(32)));
        let blocks = input.content_blocks();
        assert_eq!(blocks.len(), 3);
        assert!(
            matches!(&blocks[0], ContentBlock::Image { mime, data } if mime == "image/png" && data == "aGVsbG8")
        );
        assert!(matches!(&blocks[1], ContentBlock::Text { text } if text == "between"));
        assert!(input.has_images());
    }

    /// User-authored `[Image #…]`-like TEXT stays text — no string parsing
    /// anywhere in the manifest contract (§12).
    #[test]
    fn user_authored_placeholder_like_text_stays_text() {
        let input = TurnInput::text("[Image #99: fake]");
        assert_eq!(input.projection(), "[Image #99: fake]");
        assert_eq!(
            input.manifest(),
            vec![InputBlock::Text {
                text: "[Image #99: fake]".into()
            }]
        );
        assert!(!input.has_images());
    }
}
