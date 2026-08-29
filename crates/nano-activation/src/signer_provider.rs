//! Production receipt-signing adapter for owner-controlled external key providers.
//!
//! The adapter retains only the provider locator and public identity. Private key
//! bytes are loaded from the secured provider for the duration of one operation.

use crate::authority::KeyRole;
use crate::key_provider::{KeyProviderError, KeyReference, audit_owner_only_path};
use crate::receipt::{ReceiptError, ReceiptSigner};
use ed25519_dalek::{Signer as DalekSigner, SigningKey};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SignerProviderError {
    #[error("key reference is not for the receipt signer role")]
    RoleMismatch,
    #[error("receipt signer provider is unsupported")]
    UnsupportedProvider,
    #[error("OS receipt signer provider is unavailable")]
    OsProviderUnavailable,
    #[error("receipt signer key path is not permitted")]
    ForbiddenPath,
    #[error("receipt signer key file is unavailable")]
    Unavailable,
    #[error("receipt signer key file is not an exact Ed25519 seed")]
    InvalidKeyMaterial,
    #[error("receipt signer key changed after provider initialization")]
    KeyChanged,
    #[error("activation carrier canonicalization failed")]
    Canonicalization,
}

pub struct ExternalReceiptSigner {
    path: PathBuf,
    key_id: String,
    public_key: [u8; 32],
}

/// Owner-controlled signer used only to mint direct-CLI activation assertions.
/// It deliberately exposes no key material or provider locator.
pub struct ExternalActivationSigner {
    path: PathBuf,
    public_key: [u8; 32],
}

impl ExternalActivationSigner {
    pub fn from_key_reference(reference: &KeyReference) -> Result<Self, SignerProviderError> {
        if reference.role() != KeyRole::LocalCliIssuer {
            return Err(SignerProviderError::RoleMismatch);
        }
        match reference.provider() {
            "file" => {
                let path = PathBuf::from(reference.reference());
                validate_path(&path)?;
                let key = load_signing_key(&path)?;
                Ok(Self {
                    path,
                    public_key: key.verifying_key().to_bytes(),
                })
            }
            "os" => Err(SignerProviderError::OsProviderUnavailable),
            _ => Err(SignerProviderError::UnsupportedProvider),
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    pub fn sign_activation(
        &self,
        canonical_payload: &[u8],
    ) -> Result<[u8; 64], SignerProviderError> {
        let key = load_signing_key(&self.path)?;
        if key.verifying_key().to_bytes() != self.public_key {
            return Err(SignerProviderError::KeyChanged);
        }
        let mut message = Vec::with_capacity(30 + canonical_payload.len());
        message.extend_from_slice(b"WAYLAND-NANO-ACTIVATION\0v1\0");
        message.extend_from_slice(canonical_payload);
        Ok(key.sign(&message).to_bytes())
    }

    pub fn sign_activation_carrier(
        &self,
        carrier: &mut serde_json::Value,
    ) -> Result<(), SignerProviderError> {
        let canonical =
            serde_jcs::to_vec(carrier).map_err(|_| SignerProviderError::Canonicalization)?;
        let signature = self.sign_activation(&canonical)?;
        carrier
            .as_object_mut()
            .ok_or(SignerProviderError::Canonicalization)?
            .insert(
                "signature".into(),
                serde_json::Value::String(base64::Engine::encode(
                    &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                    signature,
                )),
            );
        Ok(())
    }
}

impl ExternalReceiptSigner {
    pub fn from_key_reference(reference: &KeyReference) -> Result<Self, SignerProviderError> {
        if reference.role() != KeyRole::ReceiptSigner {
            return Err(SignerProviderError::RoleMismatch);
        }
        match reference.provider() {
            "file" => Self::from_file_reference(reference.reference()),
            "os" => Err(SignerProviderError::OsProviderUnavailable),
            _ => Err(SignerProviderError::UnsupportedProvider),
        }
    }

    fn from_file_reference(reference: &str) -> Result<Self, SignerProviderError> {
        let path = PathBuf::from(reference);
        validate_path(&path)?;
        let signing_key = load_signing_key(&path)?;
        let public_key = signing_key.verifying_key().to_bytes();
        let fingerprint = Sha256::digest(public_key);
        let key_id = format!("receipt-ed25519-{}", hex(&fingerprint[..16]));
        Ok(Self {
            path,
            key_id,
            public_key,
        })
    }

    fn current_key(&self) -> Result<SigningKey, SignerProviderError> {
        let key = load_signing_key(&self.path)?;
        if key.verifying_key().to_bytes() != self.public_key {
            return Err(SignerProviderError::KeyChanged);
        }
        Ok(key)
    }
}

impl ReceiptSigner for ExternalReceiptSigner {
    fn key_id(&self) -> &str {
        &self.key_id
    }

    fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    fn preflight(&self) -> Result<(), ReceiptError> {
        self.current_key()
            .map(|_| ())
            .map_err(|_| ReceiptError::SignerUnavailable)
    }

    fn sign(&self, message: &[u8]) -> Result<[u8; 64], ReceiptError> {
        let key = self
            .current_key()
            .map_err(|_| ReceiptError::SignerUnavailable)?;
        Ok(key.sign(message).to_bytes())
    }
}

fn validate_path(path: &Path) -> Result<(), SignerProviderError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::Normal(name) if name == ".secrets"))
    {
        return Err(SignerProviderError::ForbiddenPath);
    }
    let text = path.as_os_str().to_string_lossy();
    if (text.starts_with("\\\\") && !is_local_extended_drive(&text)) || text.starts_with("//") {
        return Err(SignerProviderError::ForbiddenPath);
    }
    let mut parent = path.parent();
    while let Some(current) = parent {
        let metadata =
            std::fs::symlink_metadata(current).map_err(|_| SignerProviderError::Unavailable)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(SignerProviderError::ForbiddenPath);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let owner = metadata.uid();
            let mode = metadata.permissions().mode();
            if (owner != 0 && owner != unsafe { libc::geteuid() })
                || (mode & 0o022 != 0 && mode & libc::S_ISVTX as u32 == 0)
            {
                return Err(SignerProviderError::ForbiddenPath);
            }
        }
        parent = current.parent();
    }
    Ok(())
}

#[cfg(windows)]
fn is_local_extended_drive(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 7
        && value.starts_with("\\\\?\\")
        && bytes[4].is_ascii_alphabetic()
        && bytes[5] == b':'
        && bytes[6] == b'\\'
}

#[cfg(not(windows))]
fn is_local_extended_drive(_value: &str) -> bool {
    false
}

fn load_signing_key(path: &Path) -> Result<SigningKey, SignerProviderError> {
    validate_path(path)?;
    audit_owner_only_path(path).map_err(map_key_provider_error)?;
    let mut file = open_no_follow(path)?;
    let before = identity(&file)?;
    let mut seed = Vec::with_capacity(33);
    file.by_ref()
        .take(33)
        .read_to_end(&mut seed)
        .map_err(|_| SignerProviderError::Unavailable)?;
    if seed.len() != 32 {
        return Err(SignerProviderError::InvalidKeyMaterial);
    }
    let check = open_no_follow(path)?;
    if identity(&check)? != before {
        return Err(SignerProviderError::Unavailable);
    }
    let mut bytes: [u8; 32] = seed
        .try_into()
        .map_err(|_| SignerProviderError::InvalidKeyMaterial)?;
    let key = SigningKey::from_bytes(&bytes);
    bytes.fill(0);
    Ok(key)
}

fn map_key_provider_error(error: KeyProviderError) -> SignerProviderError {
    match error {
        KeyProviderError::InsecurePath | KeyProviderError::InsecurePermissions => {
            SignerProviderError::ForbiddenPath
        }
        _ => SignerProviderError::Unavailable,
    }
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> Result<File, SignerProviderError> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| SignerProviderError::Unavailable)
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> Result<File, SignerProviderError> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| SignerProviderError::Unavailable)?;
    if file
        .metadata()
        .map_err(|_| SignerProviderError::Unavailable)?
        .file_attributes()
        & 0x400
        != 0
    {
        return Err(SignerProviderError::ForbiddenPath);
    }
    Ok(file)
}

#[cfg(unix)]
fn identity(file: &File) -> Result<(u64, u64), SignerProviderError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file
        .metadata()
        .map_err(|_| SignerProviderError::Unavailable)?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn identity(file: &File) -> Result<(u64, u64), SignerProviderError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(SignerProviderError::Unavailable);
    }
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    output
}
