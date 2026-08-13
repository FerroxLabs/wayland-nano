//! Canonical image-influence walk for live and replayed history.

use nano_model::types::{ContentBlock, Message};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReplayManifestState {
    image_manifest_present: bool,
}

impl ReplayManifestState {
    pub fn from_presence(image_manifest_present: bool) -> Self {
        Self {
            image_manifest_present,
        }
    }

    pub fn is_present(self) -> bool {
        self.image_manifest_present
    }
}

pub fn history_image_influence(messages: &[Message], manifest_state: &ReplayManifestState) -> bool {
    manifest_state.is_present()
        || messages.iter().flat_map(|message| &message.content).any(|block| {
            matches!(block, ContentBlock::Image { .. })
                || matches!(block, ContentBlock::ToolResult { images, .. } if !images.is_empty())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_model::types::{ImageData, Role};

    #[test]
    fn matrix_covers_intake_result_manifest_and_clean_history() {
        assert!(!history_image_influence(
            &[],
            &ReplayManifestState::default()
        ));
        assert!(history_image_influence(
            &[Message::user_blocks(vec![ContentBlock::Image {
                mime: "image/png".into(),
                data: "AA==".into(),
            }])],
            &ReplayManifestState::default(),
        ));
        assert!(history_image_influence(
            &[Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "marker".into(),
                    is_error: false,
                    images: vec![ImageData {
                        mime: "image/png".into(),
                        data: "AA==".into()
                    }],
                }],
            }],
            &ReplayManifestState::default(),
        ));
        assert!(history_image_influence(
            &[],
            &ReplayManifestState::from_presence(true),
        ));
    }
}
