//! Skill loading and scoped activation.
//!
//! Boundary (constitution): Desktop owns discovery/catalog/trust; Nano owns
//! loading, parsing, and scoped execution context. A skill activates into a
//! turn-scoped context — name, description, and the skill's instructions —
//! and nothing else. No marketplace, no runtime dependency ecosystem.

use crate::parser::{ParsedSkillFrontmatter, parse_skill_frontmatter_metadata};
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        source: crate::parser::SkillParseError,
    },
}

/// A loaded skill: validated metadata plus its instruction body (the part of
/// SKILL.md after the frontmatter).
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub meta: ParsedSkillFrontmatter,
    pub instructions: String,
    pub source: PathBuf,
}

/// Loads one SKILL.md file, with the directory name as the default skill name.
pub fn load_skill_file(path: &Path) -> Result<Skill, SkillError> {
    let contents = std::fs::read_to_string(path)?;
    let default_name = || {
        path.parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unnamed-skill".into())
    };
    let meta = parse_skill_frontmatter_metadata(&contents, default_name)
        .map_err(|source| SkillError::Parse {
            path: path.display().to_string(),
            source,
        })?;
    let instructions = extract_body(&contents);
    Ok(Skill {
        meta,
        instructions,
        source: path.to_path_buf(),
    })
}

/// Everything after the closing frontmatter delimiter.
fn extract_body(contents: &str) -> String {
    let mut delimiters = 0;
    let mut body_start = None;
    for (index, line) in contents.lines().enumerate() {
        if line.trim() == "---" {
            delimiters += 1;
            if delimiters == 2 {
                body_start = Some(index + 1);
                break;
            }
        }
    }
    match body_start {
        Some(start) => contents.lines().skip(start).collect::<Vec<_>>().join("\n"),
        None => String::new(),
    }
}

/// Loads every skill found as `<root>/<name>/SKILL.md` under the given roots.
/// Malformed skills are collected as errors, not silently dropped — the
/// loader's caller decides whether a bad skill blocks the set.
pub fn load_skill_roots(roots: &[PathBuf]) -> (Vec<Skill>, Vec<SkillError>) {
    let mut skills = Vec::new();
    let mut errors = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let skill_file = dir.join("SKILL.md");
            if !skill_file.exists() {
                continue;
            }
            match load_skill_file(&skill_file) {
                Ok(skill) => skills.push(skill),
                Err(err) => errors.push(err),
            }
        }
    }
    skills.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    (skills, errors)
}

/// The scoped activation context injected into a turn: a compact, bounded
/// rendering of the active skills. Never the whole catalog, never secrets.
pub fn scoped_activation_context(skills: &[Skill], max_chars: usize) -> String {
    let mut out = String::from("# Active skills\n");
    for skill in skills {
        let header = format!(
            "\n## {}\n{}\n",
            skill.meta.name,
            skill
                .meta
                .short_description
                .clone()
                .unwrap_or_else(|| skill.meta.description.clone())
        );
        if out.len() + header.len() > max_chars {
            break;
        }
        out.push_str(&header);
        let remaining = max_chars.saturating_sub(out.len());
        let body: String = skill.instructions.chars().take(remaining).collect();
        out.push_str(&body);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, contents: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), contents).unwrap();
    }

    const GOOD: &str = "---\nname: fix-build\ndescription: Diagnose and fix a broken build\n---\n# Fix Build\n1. Run the build.\n2. Read the first error.\n";

    #[test]
    fn loads_good_skill_with_body() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "fix-build", GOOD);
        let skill = load_skill_file(&tmp.path().join("fix-build/SKILL.md")).unwrap();
        assert_eq!(skill.meta.name, "fix-build");
        assert_eq!(skill.meta.description, "Diagnose and fix a broken build");
        assert!(skill.instructions.contains("1. Run the build."));
    }

    #[test]
    fn directory_name_is_default_when_name_absent() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "my-skill", "---\ndescription: Has no name field\n---\nBody\n");
        let skill = load_skill_file(&tmp.path().join("my-skill/SKILL.md")).unwrap();
        assert_eq!(skill.meta.name, "my-skill");
    }

    #[test]
    fn malformed_skill_surfaces_as_error_not_silent_drop() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "bad", "no frontmatter here\n");
        let (skills, errors) = load_skill_roots(&[tmp.path().to_path_buf()]);
        assert!(skills.is_empty());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn repaired_frontmatter_with_prose_colons_loads() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "aws",
            "---\nname: aws-deploy\ndescription: Build for AWS: ECS and beyond\n---\nBody\n",
        );
        let skill = load_skill_file(&tmp.path().join("aws/SKILL.md")).unwrap();
        assert_eq!(skill.meta.name, "aws-deploy");
    }

    #[test]
    fn activation_context_is_bounded_and_scoped() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "fix-build", GOOD);
        let (skills, _) = load_skill_roots(&[tmp.path().to_path_buf()]);
        let context = scoped_activation_context(&skills, 200);
        assert!(context.starts_with("# Active skills"));
        assert!(context.contains("## fix-build"));
        assert!(context.len() <= 220);
    }
}
