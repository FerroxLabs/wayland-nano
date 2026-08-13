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
    /// §3.7/Q3: source pixel dimensions before the loader's normalization
    /// downscale, when one happened. Journaled so the replayed label
    /// re-derives byte-identically (§3.3); serde-defaulted so pre-P2b
    /// journals parse unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_from: Option<(u32, u32)>,
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
    let normalized = match image.normalized_from {
        // §3.7/Q3: surface the source geometry so first-call region
        // coordinates against the normalized raster are usable.
        Some((w, h)) => format!(" (normalized from {w}x{h})"),
        None => String::new(),
    };
    format!(
        "[Image #{} from tool {tool_name} — {}x{} {format}{normalized}]",
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
            normalized_from: image.normalized_from,
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

    /// §3.7/Q3: the label surfaces the `(normalized from WxH)` geometry iff
    /// the loader downscaled; the field is serde-defaulted in both
    /// directions so pre-P2b journals parse byte-identically and unscaled
    /// images serialize byte-minimally.
    #[test]
    fn p2b_label_surfaces_normalized_geometry_and_serde_defaults() {
        let reference = |normalized_from: Option<(u32, u32)>| ImageRef {
            digest: "a".repeat(64),
            mime: "image/png".into(),
            bytes: 3,
            width: 1024,
            height: 768,
            normalized_from,
            placeholder: String::new(),
        };
        assert_eq!(
            image_label(0, "view_image", &reference(Some((3000, 3000)))),
            "[Image #1 from tool view_image — 1024x768 png (normalized from 3000x3000)]"
        );
        assert_eq!(
            image_label(1, "view_image", &reference(None)),
            "[Image #2 from tool view_image — 1024x768 png]"
        );
        // Pre-P2b journal row (no normalized_from key) parses as None.
        let old = r#"{"digest":"aaaa","mime":"image/png","bytes":1,"width":1,"height":1,"placeholder":"p"}"#;
        let parsed: ImageRef = serde_json::from_str(old).expect("old journal parses");
        assert_eq!(parsed.normalized_from, None);
        let unscaled = serde_json::to_string(&reference(None)).expect("serialize");
        assert!(
            !unscaled.contains("normalized_from"),
            "None stays byte-minimal"
        );
        let scaled = serde_json::to_string(&reference(Some((3000, 3000)))).expect("serialize");
        assert!(scaled.contains(r#""normalized_from":[3000,3000]"#));
        // The builder carries the geometry into the journaled ref, so the
        // replayed label re-derives byte-identically (§3.3).
        let mut scaled_image = ordered(b"one", 1024);
        scaled_image.height = 768;
        scaled_image.normalized_from = Some((3000, 3000));
        let (parts, _) = build_image_tool_result("c", "view_image", vec![scaled_image]).unwrap();
        assert_eq!(parts.image_refs[0].normalized_from, Some((3000, 3000)));
        assert_eq!(
            parts.content,
            "[Image #1 from tool view_image — 1024x768 png (normalized from 3000x3000)]"
        );
    }

    /// §3.2 sealed builder: mismatched MIME/dims/count each reject BEFORE
    /// anything is journaled or dispatched (digest and order arms are pinned
    /// by the test above).
    #[test]
    fn p2b_builder_rejects_mime_dims_and_count_mismatches() {
        let mut bad_mime = ordered(b"one", 1);
        bad_mime.mime = "image/gif".into();
        assert_eq!(
            build_image_tool_result("c", "view_image", vec![bad_mime])
                .unwrap_err()
                .detail,
            "mime"
        );
        let mut bad_dims = ordered(b"one", 1);
        bad_dims.width = 0;
        assert_eq!(
            build_image_tool_result("c", "view_image", vec![bad_dims])
                .unwrap_err()
                .detail,
            "dimensions"
        );
        let too_many = (0..17)
            .map(|i| ordered(format!("n{i}").as_bytes(), 1))
            .collect::<Vec<_>>();
        assert_eq!(
            build_image_tool_result("c", "view_image", too_many)
                .unwrap_err()
                .detail,
            "image-count"
        );
    }
}
