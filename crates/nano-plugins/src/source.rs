use sha2::{Digest, Sha256};
use std::path::PathBuf;

use crate::PluginError;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RegistrySource {
    LocalDir {
        path: PathBuf,
    },
    Github {
        owner: String,
        repo: String,
        git_ref: Option<String>,
    },
}

impl RegistrySource {
    pub fn github(input: &str) -> Result<Self, PluginError> {
        if input.contains("://") || input.contains(['\\', '#', '?']) || input.starts_with('.') {
            return Err(PluginError::Invalid(
                "invalid github registry source".into(),
            ));
        }
        let (repo_path, git_ref) = match input.rsplit_once('@') {
            Some((path, r)) if !r.is_empty() => (path, Some(r.to_string())),
            Some(_) => return Err(PluginError::Invalid("github ref cannot be empty".into())),
            None => (input, None),
        };
        let mut parts = repo_path.split('/');
        let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(PluginError::Invalid(
                "github source must be owner/repo[@ref]".into(),
            ));
        };
        if !valid_segment(owner)
            || !valid_segment(repo)
            || git_ref.as_deref().is_some_and(str::is_empty)
        {
            return Err(PluginError::Invalid(
                "invalid github owner, repo, or ref".into(),
            ));
        }
        Ok(Self::Github {
            owner: owner.into(),
            repo: repo.into(),
            git_ref,
        })
    }

    pub fn reg_key(&self) -> String {
        let canonical = serde_json::to_vec(self).expect("registry source serializes");
        let digest = format!("{:x}", Sha256::digest(canonical));
        format!("reg_{}", &digest[..16])
    }

    pub fn display(&self) -> String {
        match self {
            Self::LocalDir { path } => format!("local-dir {}", path.display()),
            Self::Github {
                owner,
                repo,
                git_ref,
            } => format!(
                "github {owner}/{repo}{}",
                git_ref
                    .as_ref()
                    .map(|r| format!("@{r}"))
                    .unwrap_or_default()
            ),
        }
    }
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_matrix() {
        assert!(matches!(
            RegistrySource::github("o/r").unwrap(),
            RegistrySource::Github { git_ref: None, .. }
        ));
        assert!(matches!(
            RegistrySource::github("o/r@abc123").unwrap(),
            RegistrySource::Github {
                git_ref: Some(_),
                ..
            }
        ));
        for bad in [
            "https://github.com/o/r",
            "o/r@",
            "o/r#x",
            r"C:\repo",
            "o/r/x",
        ] {
            assert!(RegistrySource::github(bad).is_err(), "{bad}");
        }
    }
}
