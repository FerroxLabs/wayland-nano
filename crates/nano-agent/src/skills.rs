//! Skills activation for turns: load skill roots and render the scoped
//! activation context as a system message.
//!
//! Boundary: Desktop owns discovery/catalog/trust; Nano loads, parses, and
//! scopes. Activation is one system message prepended to the turn — bounded,
//! never the whole catalog, never secrets.

use nano_model::types::{ContentBlock, Message, Role};
use nano_skills::loader::{Skill, load_skill_roots, scoped_activation_context};
use std::path::PathBuf;

const ACTIVATION_BUDGET_CHARS: usize = 8_000;

/// Loads skills from the given roots and returns the scoped activation
/// system message, or None when no valid skills exist (or all failed —
/// failures are logged to stderr, never silently dropped).
pub fn prepare_skill_context(roots: &[PathBuf]) -> Option<Message> {
    let (skills, errors) = load_skill_roots(roots);
    for error in &errors {
        eprintln!("nanok3: skill load error: {error}");
    }
    if skills.is_empty() {
        return None;
    }
    Some(activation_message(&skills))
}

fn activation_message(skills: &[Skill]) -> Message {
    let context = scoped_activation_context(skills, ACTIVATION_BUDGET_CHARS);
    Message {
        role: Role::System,
        content: vec![ContentBlock::Text { text: context }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_message_when_skills_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fix-build");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: fix-build\ndescription: Fix broken builds\n---\n1. Run build.\n",
        )
        .unwrap();
        let message = prepare_skill_context(&[tmp.path().to_path_buf()]).expect("context");
        let ContentBlock::Text { text } = &message.content[0] else {
            panic!()
        };
        assert!(text.contains("## fix-build"));
        assert!(matches!(message.role, Role::System));
    }

    #[test]
    fn no_message_without_valid_skills() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(prepare_skill_context(&[tmp.path().to_path_buf()]).is_none());
    }
}
