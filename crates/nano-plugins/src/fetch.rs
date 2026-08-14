use crate::{PluginError, manifest::validate_relative, source::RegistrySource};
use flate2::read::GzDecoder;
use nano_egress::{client::EgressClient, policy::EgressPolicy};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_COMPRESSED: usize = 64 * 1024 * 1024;
const MAX_EXTRACTED: u64 = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

pub struct FetchedRegistry {
    pub root: PathBuf,
    pub resolved_ref: String,
    pub archive_sha256: String,
}

pub async fn fetch(
    source: &RegistrySource,
    cache_root: &Path,
) -> Result<FetchedRegistry, PluginError> {
    match source {
        RegistrySource::LocalDir { path } => {
            let root = fs::canonicalize(path).map_err(|e| PluginError::io(path, e))?;
            if !root.is_dir() || !root.join("marketplace.json").is_file() {
                return Err(PluginError::Invalid(
                    "local registry lacks marketplace.json".into(),
                ));
            }
            let digest = hash_tree(&root)?;
            Ok(FetchedRegistry {
                root,
                resolved_ref: "local".into(),
                archive_sha256: digest,
            })
        }
        RegistrySource::Github {
            owner,
            repo,
            git_ref,
        } => fetch_github(owner, repo, git_ref.as_deref(), cache_root).await,
    }
}

async fn fetch_github(
    owner: &str,
    repo: &str,
    git_ref: Option<&str>,
    cache_root: &Path,
) -> Result<FetchedRegistry, PluginError> {
    let policy = EgressPolicy::new()
        .allow_host("codeload.github.com")
        .allow_host("github.com");
    let client = EgressClient::without_redirects(policy);
    let resolved = if let Some(r) = git_ref {
        r.to_string()
    } else {
        probe_latest(&client, owner, repo)
            .await?
            .unwrap_or_else(|| "HEAD".into())
    };
    let url = format!("https://codeload.github.com/{owner}/{repo}/tar.gz/{resolved}");
    let mut response = client
        .request(reqwest::Method::GET, &url)?
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| PluginError::Http(client.classify_transport(&e).to_string()))?;
    if !response.status().is_success() {
        return Err(client
            .classify_status(&url, response.status().as_u16())
            .into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_COMPRESSED as u64)
    {
        return Err(PluginError::ArchiveBound);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| PluginError::Http(client.classify_transport(&e).to_string()))?
    {
        if bytes.len() + chunk.len() > MAX_COMPRESSED {
            return Err(PluginError::ArchiveBound);
        }
        bytes.extend_from_slice(&chunk);
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    fs::create_dir_all(cache_root).map_err(|e| PluginError::io(cache_root, e))?;
    let staging = cache_root.with_extension(format!("staging-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| PluginError::io(&staging, e))?;
    }
    fs::create_dir_all(&staging).map_err(|e| PluginError::io(&staging, e))?;
    let result = extract(&bytes, &staging);
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    let entries = fs::read_dir(&staging)
        .map_err(|e| PluginError::io(&staging, e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| PluginError::io(&staging, e))?;
    let extracted = if entries.len() == 1
        && entries[0]
            .file_type()
            .map_err(|e| PluginError::io(entries[0].path(), e))?
            .is_dir()
    {
        entries[0].path()
    } else {
        staging.clone()
    };
    if cache_root.exists() {
        fs::remove_dir_all(cache_root).map_err(|e| PluginError::io(cache_root, e))?;
    }
    if extracted == staging {
        fs::rename(&staging, cache_root).map_err(|e| PluginError::io(cache_root, e))?;
    } else {
        fs::rename(&extracted, cache_root).map_err(|e| PluginError::io(cache_root, e))?;
        let _ = fs::remove_dir_all(&staging);
    }
    Ok(FetchedRegistry {
        root: cache_root.into(),
        resolved_ref: resolved,
        archive_sha256: digest,
    })
}
async fn probe_latest(
    client: &EgressClient,
    owner: &str,
    repo: &str,
) -> Result<Option<String>, PluginError> {
    let url = format!("https://github.com/{owner}/{repo}/releases/latest");
    let response = client
        .request(reqwest::Method::GET, &url)?
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| PluginError::Http(client.classify_transport(&e).to_string()))?;
    if response.status().is_redirection() {
        let tag = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|x| x.to_str().ok())
            .and_then(|x| x.rsplit("/tag/").next())
            .filter(|x| !x.contains('/'));
        return Ok(tag.map(str::to_string));
    }
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if response.status().is_success() {
        return Ok(None);
    }
    Err(client
        .classify_status(&url, response.status().as_u16())
        .into())
}
fn extract(bytes: &[u8], dest: &Path) -> Result<(), PluginError> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    let mut count = 0;
    let mut total = 0;
    for item in archive.entries().map_err(|e| PluginError::io(dest, e))? {
        let mut entry = item.map_err(|e| PluginError::io(dest, e))?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err(PluginError::ArchiveBound);
        }
        let path = entry
            .path()
            .map_err(|e| PluginError::io(dest, e))?
            .into_owned();
        validate_relative(&path)?;
        let ty = entry.header().entry_type();
        if !(ty.is_file() || ty.is_dir()) {
            return Err(PluginError::UnsafePath(path));
        }
        total += entry
            .header()
            .size()
            .map_err(|e| PluginError::io(dest, e))?;
        if total > MAX_EXTRACTED {
            return Err(PluginError::ArchiveBound);
        }
        entry
            .unpack_in(dest)
            .map_err(|e| PluginError::io(dest, e))?;
    }
    Ok(())
}
fn hash_tree(root: &Path) -> Result<String, PluginError> {
    fn walk(root: &Path, dir: &Path, h: &mut Sha256) -> Result<(), PluginError> {
        let mut es = fs::read_dir(dir)
            .map_err(|e| PluginError::io(dir, e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PluginError::io(dir, e))?;
        es.sort_by_key(|e| e.file_name());
        for e in es {
            let ft = e.file_type().map_err(|x| PluginError::io(e.path(), x))?;
            if ft.is_symlink() {
                continue;
            }
            let rel = e
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            h.update(rel.as_bytes());
            if ft.is_dir() {
                walk(root, &e.path(), h)?
            } else if ft.is_file() {
                let mut f = fs::File::open(e.path()).map_err(|x| PluginError::io(e.path(), x))?;
                let mut b = [0; 8192];
                loop {
                    let n = f.read(&mut b).map_err(|x| PluginError::io(e.path(), x))?;
                    if n == 0 {
                        break;
                    }
                    h.update(&b[..n]);
                }
            }
        }
        Ok(())
    }
    let mut h = Sha256::new();
    walk(root, root, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}
