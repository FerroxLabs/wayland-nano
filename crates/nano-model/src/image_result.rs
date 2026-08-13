//! Canonical construction and replay verification for image-bearing tool results.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::ImageData;

const MAX_IMAGES: usize = 16;
const MAX_DIMENSION: u32 = 2_000;
const MAX_PIXELS: u64 = 1_048_576;
const MAX_RAW_BYTES: u64 = 589_824;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    pub digest: String,
    pub mime: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub placeholder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedImage {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub digest: String,
    pub width: u32,
    pub height: u32,
    pub normalized_from: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageToolResultParts {
    pub content: String,
    pub images: Vec<ImageData>,
    pub image_refs: Vec<ImageRef>,
    pub output_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProvenanceKind<'a> {
    Live,
    ReplayVerified { digest: &'a str },
}

#[must_use = "image provenance must be consumed at the live or replay acceptance seam"]
#[derive(Debug)]
pub struct ImageProvenance {
    inner: Provenance,
}

#[derive(Debug)]
enum Provenance {
    Live,
    ReplayVerified { digest: String },
}

impl ImageProvenance {
    pub fn kind(&self) -> ImageProvenanceKind<'_> {
        match &self.inner {
            Provenance::Live => ImageProvenanceKind::Live,
            Provenance::ReplayVerified { digest } => ImageProvenanceKind::ReplayVerified { digest },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageError {
    pub detail: &'static str,
}

fn mint_live() -> ImageProvenance {
    ImageProvenance {
        inner: Provenance::Live,
    }
}

fn mint_replay(digest: String) -> ImageProvenance {
    ImageProvenance {
        inner: Provenance::ReplayVerified { digest },
    }
}

pub fn image_label(index: usize, tool_name: &str, image: &ImageRef) -> String {
    let format = image.mime.strip_prefix("image/").unwrap_or("image");
    format!(
        "[Image #{} from tool {tool_name} — {}x{} {format}]",
        index + 1,
        image.width,
        image.height
    )
}

pub fn build_image_tool_result(
    _call_id: &str,
    tool_name: &str,
    ordered_images: Vec<OrderedImage>,
) -> Result<(ImageToolResultParts, ImageProvenance), ImageError> {
    validate_count(ordered_images.len())?;
    let mut refs = Vec::with_capacity(ordered_images.len());
    let mut images = Vec::with_capacity(ordered_images.len());
    for image in ordered_images {
        validate_image(&image)?;
        refs.push(ImageRef {
            digest: image.digest,
            mime: image.mime.clone(),
            bytes: image.bytes.len() as u64,
            width: image.width,
            height: image.height,
            placeholder: String::new(),
        });
        images.push(ImageData {
            mime: image.mime,
            data: base64::engine::general_purpose::STANDARD.encode(image.bytes),
        });
    }
    for (index, reference) in refs.iter_mut().enumerate() {
        reference.placeholder = image_label(index, tool_name, reference);
    }
    let content = refs
        .iter()
        .enumerate()
        .map(|(index, reference)| image_label(index, tool_name, reference))
        .collect::<Vec<_>>()
        .join("\n");
    let output_digest = hex_sha256(content.as_bytes());
    Ok((
        ImageToolResultParts {
            content,
            images,
            image_refs: refs,
            output_digest,
        },
        mint_live(),
    ))
}

pub fn rehydrate_tool_result_images(
    verified: Vec<(ImageRef, Vec<u8>)>,
) -> Result<(Vec<ImageData>, ImageProvenance), ImageError> {
    validate_count(verified.len())?;
    let mut images = Vec::with_capacity(verified.len());
    let mut aggregate = Sha256::new();
    for (reference, bytes) in verified {
        let image = OrderedImage {
            bytes,
            mime: reference.mime,
            digest: reference.digest,
            width: reference.width,
            height: reference.height,
            normalized_from: None,
        };
        validate_image(&image)?;
        aggregate.update(image.digest.as_bytes());
        images.push(ImageData {
            mime: image.mime,
            data: base64::engine::general_purpose::STANDARD.encode(image.bytes),
        });
    }
    Ok((images, mint_replay(format!("{:x}", aggregate.finalize()))))
}

fn validate_count(count: usize) -> Result<(), ImageError> {
    if count == 0 || count > MAX_IMAGES {
        return Err(ImageError {
            detail: "image-count",
        });
    }
    Ok(())
}

fn validate_image(image: &OrderedImage) -> Result<(), ImageError> {
    if !matches!(image.mime.as_str(), "image/png" | "image/jpeg") {
        return Err(ImageError { detail: "mime" });
    }
    if image.width == 0
        || image.height == 0
        || image.width > MAX_DIMENSION
        || image.height > MAX_DIMENSION
        || u64::from(image.width) * u64::from(image.height) > MAX_PIXELS
    {
        return Err(ImageError {
            detail: "dimensions",
        });
    }
    if image.bytes.is_empty() || image.bytes.len() as u64 > MAX_RAW_BYTES {
        return Err(ImageError { detail: "payload" });
    }
    if hex_sha256(&image.bytes) != image.digest {
        return Err(ImageError { detail: "digest" });
    }
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordered(bytes: &[u8], width: u32) -> OrderedImage {
        OrderedImage {
            bytes: bytes.to_vec(),
            mime: "image/png".into(),
            digest: hex_sha256(bytes),
            width,
            height: 1,
            normalized_from: None,
        }
    }

    #[test]
    fn builder_derives_projection_images_refs_and_digest() {
        let (parts, provenance) = build_image_tool_result(
            "c1",
            "view_image",
            vec![ordered(b"one", 1), ordered(b"two", 2)],
        )
        .unwrap();
        assert!(matches!(provenance.kind(), ImageProvenanceKind::Live));
        assert_eq!(parts.images.len(), 2);
        assert_eq!(
            parts.image_refs.iter().map(|r| r.width).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(parts.output_digest, hex_sha256(parts.content.as_bytes()));
        assert_eq!(parts.content.lines().count(), 2);
    }

    #[test]
    fn mismatch_is_rejected_and_order_changes_both_views() {
        let mut bad = ordered(b"bad", 1);
        bad.digest = "0".repeat(64);
        assert_eq!(
            build_image_tool_result("c", "view_image", vec![bad])
                .unwrap_err()
                .detail,
            "digest"
        );
        let (a, _) = build_image_tool_result(
            "c",
            "view_image",
            vec![ordered(b"one", 1), ordered(b"two", 2)],
        )
        .unwrap();
        let (b, _) = build_image_tool_result(
            "c",
            "view_image",
            vec![ordered(b"two", 2), ordered(b"one", 1)],
        )
        .unwrap();
        assert_ne!(a.content, b.content);
        assert_ne!(a.image_refs, b.image_refs);
    }
}
