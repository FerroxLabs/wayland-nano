//! P2a §4 — the hardened image loader: the ONE canonical intake for image
//! bytes, host-side (§3.3). Ported from grok's
//! `xai-grok-tools/src/util/image_validate.rs` (UPSTREAM.md ledger entry),
//! hardened per the P2a design note:
//!
//! - magic-byte sniffing ONLY; the sender's MIME claim and path extension
//!   are diagnostics, never trusted — claim-vs-sniff mismatch is a typed
//!   `ImageInvalid`, never a quiet re-label (§4.1);
//! - the §4.2 cap table (binary units, checked/saturating arithmetic);
//! - header-dimensions-before-decode: the decompression-bomb guard rejects
//!   on the header probe BEFORE any pixel allocation (§4.3 step 3);
//! - GIF/WebP first-frame-only with a bounded canvas (§4.4);
//! - decode inside `spawn_blocking` with `catch_unwind(AssertUnwindSafe(..))`
//!   INSIDE the blocking closure plus `JoinError::is_panic()` mapping
//!   (defense-in-depth, §4.3 step 4 / D11) — under the workspace
//!   `panic = "unwind"` profile, asserted by a test below;
//! - orientation applied, EXIF/GPS stripped BY CONSTRUCTION: the encoders
//!   receive only the freshly allocated pixel buffer (§4.5);
//! - sequential decode: a process-wide 1-permit semaphore bounds transient
//!   decode memory to the documented ~200 MiB per-decode ceiling (§4.2).
//!
//! Output (§4.6): re-encoded bytes (png/jpeg only — a strict subset of the
//! closed intake set), the sniffed source mime, the sha256 digest of the
//! re-encoded bytes, final dimensions, and the frame-drop flag. Originals
//! are read once and dropped; only re-encoded bytes may reach the blob
//! store or the wire.

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};

use nano_session::NanoErrorKind;
use sha2::{Digest, Sha256};

// ── §4.2 cap table — binary units (1 KiB = 1,024 B), checked/saturating
// arithmetic throughout. ──────────────────────────────────────────────────

/// Pre-decode intake ceiling per file (grok `MAX_SEND_BYTES`). Bounds read
/// I/O before any decoder runs.
pub const MAX_IMAGE_FILE_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB = 52,428,800 B
/// Per-prompt image count cap (grok `placeholder_images.rs`).
pub const MAX_IMAGES_PER_PROMPT: usize = 16;
/// Per-prompt aggregate cap over input file sizes (kills the 16 × 50 MiB
/// read bomb). Saturating-add accounting.
pub const MAX_PROMPT_IMAGE_AGGREGATE_BYTES: u64 = MAX_IMAGE_FILE_BYTES;
/// Edge cap; over-edge images are DOWNSCALED, not rejected.
pub const MAX_IMAGE_DIMENSION: u32 = 2_000;
/// The NORMALIZATION TARGET after downscale (≈1024×1024) — what the model
/// sees. NOT the decode ceiling (§14 deviation 3).
pub const MAX_IMAGE_PIXELS: u64 = 1_048_576;
/// Memory-budget decode ceiling [r2 codex-F1]: 33,554,432 px × 4 B/px =
/// 128 MiB worst-case RGBA per decode, + ≤50 MiB compressed input + bounded
/// decoder scratch ⇒ the documented ~200 MiB per-decode ceiling.
pub const MAX_DECODE_PIXELS: u64 = 33_554_432;
/// Per-dimension sanity bound (header-integer abuse guard).
pub const MAX_DECODE_DIMENSION: u32 = 65_535;
/// The documented per-decode memory ceiling; process-test-asserted (§12).
pub const MAX_PER_DECODE_MEMORY_BYTES: u64 = 200 * 1024 * 1024;
/// Wire payload cap: 768 KiB of BASE64 (grok). Raw bytes ≈ 576 KiB.
pub const MAX_IMAGE_PAYLOAD_BYTES: u64 = 768 * 1024; // 786,432 B base64
/// Raw-byte ceiling implied by the base64 payload cap: floor(786432/4)*3.
pub const MAX_IMAGE_RAW_PAYLOAD_BYTES: u64 = MAX_IMAGE_PAYLOAD_BYTES / 4 * 3; // 589,824 B
/// grok's re-encode ladder; ladder-floor overflow is a typed ImageTooLarge.
pub const JPEG_QUALITY_STEPS: [u8; 8] = [88, 80, 72, 64, 56, 48, 40, 32];

/// The wire mimes the loader can emit — a strict SUBSET of the closed
/// intake set (§4.5).
pub const WIRE_MIME_PNG: &str = "image/png";
pub const WIRE_MIME_JPEG: &str = "image/jpeg";

/// A typed loader rejection. `kind` is the closed journal vocabulary (C7);
/// `detail` is a bounded-vocabulary tag naming WHICH cap/check fired — never
/// provider text, paths, or free-form content (the `nanoError`
/// closed-fields rule, P2a §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageError {
    pub kind: NanoErrorKind,
    pub detail: &'static str,
}

impl ImageError {
    fn invalid(detail: &'static str) -> Self {
        Self {
            kind: NanoErrorKind::ImageInvalid,
            detail,
        }
    }
    fn unsupported(detail: &'static str) -> Self {
        Self {
            kind: NanoErrorKind::ImageUnsupportedFormat,
            detail,
        }
    }
    fn too_large(detail: &'static str) -> Self {
        Self {
            kind: NanoErrorKind::ImageTooLarge,
            detail,
        }
    }
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = serde_json::to_value(self.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_string());
        write!(f, "{kind}: {}", self.detail)
    }
}

impl std::error::Error for ImageError {}

/// The closed intake set, sniffed from magic bytes (§4.1/§4.3 step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SniffedFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
}

impl SniffedFormat {
    /// The sniffed source mime (§4.6) — never the sender's claim.
    pub fn mime(self) -> &'static str {
        match self {
            SniffedFormat::Png => "image/png",
            SniffedFormat::Jpeg => "image/jpeg",
            SniffedFormat::Gif => "image/gif",
            SniffedFormat::WebP => "image/webp",
        }
    }
}

/// §4.6 loader output. `bytes` are RE-ENCODED pixels (png/jpeg) — never the
/// originals — and are the only bytes that may reach the §5 blob store or
/// the wire.
#[derive(Debug, Clone)]
pub struct LoadedImage {
    /// Re-encoded bytes (the digest below is over these).
    pub bytes: Vec<u8>,
    /// Output mime, from the closed wire subset {image/png, image/jpeg}.
    pub wire_mime: String,
    /// Sniffed SOURCE mime (§4.1), recorded for diagnostics/receipt.
    pub sniffed_mime: String,
    /// Lowercase hex sha256 of `bytes` — the §5 blob-store content address.
    pub digest: String,
    /// TUI-path provenance, filled by the host caller (the loader never
    /// touches the filesystem policy itself; §3.3).
    pub orig_path: Option<String>,
    /// The display placeholder (`[Image #N: <path>]`), minted by the caller
    /// — the loader has no session state to number placeholders.
    pub placeholder: Option<String>,
    /// Final (post-orientation, post-downscale) pixel dimensions.
    pub width: u32,
    pub height: u32,
    /// GIF/WebP carried more than one frame; only the first crossed (§4.4).
    pub frames_dropped: bool,
}

impl LoadedImage {
    /// §9.4 intake receipt: ONE log line per accepted image — placeholder,
    /// sniffed mime, dims, frame drop, re-encoded bytes, digest prefix, caps
    /// headroom. The host logs this; the loader never logs itself.
    pub fn receipt_line(&self) -> String {
        let placeholder = self.placeholder.as_deref().unwrap_or("[image]");
        let frames = if self.frames_dropped {
            "frames: N -> 1"
        } else {
            "frames: 1"
        };
        let headroom = MAX_IMAGE_RAW_PAYLOAD_BYTES.saturating_sub(self.bytes.len() as u64);
        format!(
            "{placeholder} sniffed={} dims={}x{} {frames} reencoded={}B digest={} payload_headroom={headroom}B",
            self.sniffed_mime,
            self.width,
            self.height,
            self.bytes.len(),
            &self.digest[..12],
        )
    }
}

/// Sequential decode bound (§4.2): ONE decode at a time per process, so a
/// 16-image prompt peaks at ~200 MiB transient, not ~10 GiB.
static DECODE_SEMAPHORE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Decode instrumentation (the §12 "allocation-instrumented"/sequential
/// proofs): the decode body bumps these; tests assert the header-probe
/// rejections NEVER enter the decode body and concurrency never exceeds 1.
static ACTIVE_DECODES: AtomicUsize = AtomicUsize::new(0);
static MAX_OBSERVED_ACTIVE_DECODES: AtomicUsize = AtomicUsize::new(0);
static TOTAL_DECODES: AtomicUsize = AtomicUsize::new(0);

/// Test hook for the §12 panic battery: when non-zero, the decode body
/// panics mid-decode for the input whose length matches (a poisoned-decoder
/// shim). Length-targeted so parallel tests never steal each other's poison.
#[cfg(test)]
static POISON_DECODE_LEN: AtomicUsize = AtomicUsize::new(0);

/// The §4.3 pipeline, steps 1–6. `claimed_mime` is the sender's ACP
/// `mimeType` hint — a HINT ONLY; a claim-vs-sniff mismatch is a typed
/// rejection (§4.1). Pass `None` on the TUI/path path (extension is a
/// pre-filter owned by the host, never trusted either).
///
/// Async only for the `spawn_blocking` boundary; all validation before the
/// decode is synchronous and allocation-bounded.
pub async fn load_image(
    bytes: &[u8],
    claimed_mime: Option<&str>,
) -> Result<LoadedImage, ImageError> {
    // Step 1: intake byte ceiling, before any decoder runs.
    if bytes.len() as u64 > MAX_IMAGE_FILE_BYTES {
        return Err(ImageError::too_large("file-bytes"));
    }
    // Step 2: magic-byte sniff → closed decoder allowlist.
    let format = sniff(bytes)?;
    // Step 2b: claim-vs-sniff mismatch is a typed rejection, never a
    // quiet re-label.
    if let Some(claim) = claimed_mime {
        check_claim(claim, format)?;
    }
    // Step 3: header-only dimension probe BEFORE any pixel allocation —
    // THE decompression-bomb guard. Contained like the decode (a hostile
    // header must not unwind into the async runtime either).
    let probe_bytes = bytes.to_vec();
    let dims = catch_unwind(AssertUnwindSafe(|| header_dimensions(&probe_bytes, format)))
        .map_err(|_| ImageError::invalid("header-probe-panic"))??;
    check_decode_dimensions(dims.0, dims.1)?;

    // Steps 4–6: full decode + frame policy + orientation/re-encode inside
    // spawn_blocking, panic-contained (D11). The permit is held for the
    // whole blocking closure and released on every exit path, including
    // cancellation of the outer future (drop) and contained panics.
    let owned = bytes.to_vec();
    let permit = DECODE_SEMAPHORE.acquire().await.map_err(|_| ImageError {
        kind: NanoErrorKind::UserCancelled,
        detail: "decode-semaphore-closed",
    })?;
    let join = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        catch_unwind(AssertUnwindSafe(|| {
            decode_normalize_reencode(&owned, format)
        }))
    })
    .await;
    match join {
        // An unwinding decoder panic became Ok(Err(..)) inside the closure.
        Ok(Ok(result)) => result,
        Ok(Err(_panic_payload)) => Err(ImageError::invalid("decoder-panic")),
        Err(join_err) => Err(map_join_error(join_err)),
    }
}

/// §4.3 step 4 outer mapping: a JoinError is panic (defense-in-depth —
/// normally already contained inside the closure) or cancellation.
fn map_join_error(err: tokio::task::JoinError) -> ImageError {
    if err.is_panic() {
        ImageError::invalid("decoder-panic")
    } else if err.is_cancelled() {
        ImageError {
            kind: NanoErrorKind::UserCancelled,
            detail: "decode-cancelled",
        }
    } else {
        // JoinError has no third failure mode; classify conservatively.
        ImageError::invalid("decoder-join")
    }
}

/// §4.1: format detection by magic bytes ONLY, into the closed allowlist.
/// SVG is excluded by construction (no decoder is linked — D8) and gets its
/// own typed rejection; BMP/TIFF are outside the P2a intake set (§14
/// deviation 7) with a convert-and-retry hint.
fn sniff(bytes: &[u8]) -> Result<SniffedFormat, ImageError> {
    if looks_like_svg(bytes) {
        return Err(ImageError::unsupported("svg"));
    }
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => Ok(SniffedFormat::Png),
        Ok(image::ImageFormat::Jpeg) => Ok(SniffedFormat::Jpeg),
        Ok(image::ImageFormat::Gif) => Ok(SniffedFormat::Gif),
        Ok(image::ImageFormat::WebP) => Ok(SniffedFormat::WebP),
        Ok(_) => Err(ImageError::unsupported("outside-closed-set")),
        Err(_) => Err(ImageError::invalid("sniff-failure")),
    }
}

/// Text formats never sniff as a raster; SVG gets
/// `ImageUnsupportedFormat` rather than `ImageInvalid` (§12 battery).
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let trimmed = text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_lowercase();
    trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && trimmed.contains("<svg"))
}

/// The claimed mime (ACP `mimeType`) is a HINT; mismatch with the sniffed
/// format is a typed `ImageInvalid` (§4.1, grok's WrongFormat arm).
fn check_claim(claim: &str, sniffed: SniffedFormat) -> Result<(), ImageError> {
    let claim = claim.trim().to_ascii_lowercase();
    let claimed = match claim.as_str() {
        "image/png" => Some(SniffedFormat::Png),
        "image/jpeg" => Some(SniffedFormat::Jpeg),
        "image/gif" => Some(SniffedFormat::Gif),
        "image/webp" => Some(SniffedFormat::WebP),
        _ => None,
    };
    match claimed {
        Some(format) if format == sniffed => Ok(()),
        Some(_) => Err(ImageError::invalid("mime-claim-mismatch")),
        None => Err(ImageError::invalid("mime-claim-unknown")),
    }
}

/// §4.3 step 3: header-only probe (no pixel allocation). Delegates to the
/// `image` crate's header readers (PNG IHDR, JPEG SOF walk, GIF logical-
/// screen descriptor, WebP VP8/VP8L/VP8X canvas) — the same probes the note
/// enumerates, without re-implementing them.
fn header_dimensions(bytes: &[u8], format: SniffedFormat) -> Result<(u32, u32), ImageError> {
    let image_format = match format {
        SniffedFormat::Png => image::ImageFormat::Png,
        SniffedFormat::Jpeg => image::ImageFormat::Jpeg,
        SniffedFormat::Gif => image::ImageFormat::Gif,
        SniffedFormat::WebP => image::ImageFormat::WebP,
    };
    image::ImageReader::with_format(Cursor::new(bytes), image_format)
        .into_dimensions()
        .map_err(|_| ImageError::invalid("header-parse"))
}

/// The bomb guard: dimension product via checked arithmetic, per-dimension
/// sanity bound, then the memory-derived pixel ceiling — ALL before the
/// decoder allocates (§4.3 step 3).
fn check_decode_dimensions(w: u32, h: u32) -> Result<(), ImageError> {
    if w == 0 || h == 0 {
        return Err(ImageError::invalid("zero-dimension"));
    }
    let Some(pixels) = w.checked_mul(h) else {
        return Err(ImageError::too_large("dimension-overflow"));
    };
    if w > MAX_DECODE_DIMENSION || h > MAX_DECODE_DIMENSION {
        return Err(ImageError::too_large("dimension"));
    }
    if u64::from(pixels) > MAX_DECODE_PIXELS {
        return Err(ImageError::too_large("decode-pixels"));
    }
    Ok(())
}

/// §4.2 count/aggregate caps for one prompt, saturating-add accounting.
/// `sizes` are the input file sizes (pre-decode), one per image.
pub fn check_prompt_limits(sizes: impl IntoIterator<Item = u64>) -> Result<(), ImageError> {
    let mut count = 0usize;
    let mut aggregate = 0u64;
    for size in sizes {
        count = count.saturating_add(1);
        aggregate = aggregate.saturating_add(size);
    }
    if count > MAX_IMAGES_PER_PROMPT {
        return Err(ImageError {
            kind: NanoErrorKind::ImageTooMany,
            detail: "count",
        });
    }
    if aggregate > MAX_PROMPT_IMAGE_AGGREGATE_BYTES {
        return Err(ImageError {
            kind: NanoErrorKind::ImageTooMany,
            detail: "aggregate",
        });
    }
    Ok(())
}

/// Host-side helper for the TUI/path intake (§3.3): read a file bounded by
/// `MAX_IMAGE_FILE_BYTES` (+1 to detect overflow) — the byte ceiling fires
/// with ZERO decode. Path POLICY (canonicalize/allowlist/sensitive-subtree)
/// is the host's policy layer, not this module's.
pub fn read_image_file_capped(path: &std::path::Path) -> Result<Vec<u8>, ImageError> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| ImageError {
        kind: NanoErrorKind::FsIo,
        detail: "open",
    })?;
    let mut limited = file.take(MAX_IMAGE_FILE_BYTES.saturating_add(1));
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes).map_err(|_| ImageError {
        kind: NanoErrorKind::FsIo,
        detail: "read",
    })?;
    if bytes.len() as u64 > MAX_IMAGE_FILE_BYTES {
        return Err(ImageError::too_large("file-bytes"));
    }
    Ok(bytes)
}

/// §4.3 steps 4–6, running inside the contained blocking closure.
fn decode_normalize_reencode(
    bytes: &[u8],
    format: SniffedFormat,
) -> Result<LoadedImage, ImageError> {
    let active = ACTIVE_DECODES.fetch_add(1, Ordering::SeqCst) + 1;
    MAX_OBSERVED_ACTIVE_DECODES.fetch_max(active, Ordering::SeqCst);
    TOTAL_DECODES.fetch_add(1, Ordering::SeqCst);
    let guard = scopeguard_decode_counter();
    let out = decode_inner(bytes, format);
    drop(guard);
    out
}

fn scopeguard_decode_counter() -> impl Drop {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            ACTIVE_DECODES.fetch_sub(1, Ordering::SeqCst);
        }
    }
    Guard
}

fn decode_inner(bytes: &[u8], format: SniffedFormat) -> Result<LoadedImage, ImageError> {
    #[cfg(test)]
    {
        let target = POISON_DECODE_LEN.load(Ordering::SeqCst);
        if target != 0 && target == bytes.len() {
            POISON_DECODE_LEN.store(0, Ordering::SeqCst);
            panic!("poisoned decoder shim (§12 panic battery)");
        }
    }

    // §4.4 frame policy: GIF and animated WebP decode the FIRST FRAME ONLY
    // (frame iterators are never collected); the canvas was already bounded
    // by the step-3 header probe.
    let (decoded, frames_dropped) = match format {
        SniffedFormat::Gif => {
            use image::AnimationDecoder as _;
            let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
                .map_err(|_| ImageError::invalid("gif-decode"))?;
            let mut frames = decoder.into_frames();
            let first = frames
                .next()
                .ok_or_else(|| ImageError::invalid("gif-no-frames"))?
                .map_err(|_| ImageError::invalid("gif-decode"))?;
            // Drive the iterator at most ONE step further — enough to
            // disclose the drop, never a collection.
            let dropped = match frames.next() {
                Some(Ok(_)) => true,
                Some(Err(_)) => return Err(ImageError::invalid("gif-decode")),
                None => false,
            };
            (
                image::DynamicImage::ImageRgba8(first.into_buffer()),
                dropped,
            )
        }
        SniffedFormat::WebP => {
            use image::ImageDecoder as _;
            let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                .map_err(|_| ImageError::invalid("webp-decode"))?;
            let dropped = decoder.has_animation();
            let (w, h) = decoder.dimensions();
            let color = decoder.color_type();
            // The header probe already bounded w*h ≤ MAX_DECODE_PIXELS, so
            // this canvas allocation is ceiling-bounded by construction.
            let mut buf = vec![
                0u8;
                usize::try_from(decoder.total_bytes())
                    .map_err(|_| ImageError::too_large("decode-pixels"))?
            ];
            // image-webp read_image renders the FIRST frame onto the canvas
            // for animated sources; later frames never decode (§4.4).
            decoder
                .read_image(&mut buf)
                .map_err(|_| ImageError::invalid("webp-decode"))?;
            let img = match color {
                image::ColorType::Rgba8 => {
                    image::RgbaImage::from_raw(w, h, buf).map(image::DynamicImage::ImageRgba8)
                }
                _ => image::RgbImage::from_raw(w, h, buf).map(image::DynamicImage::ImageRgb8),
            }
            .ok_or_else(|| ImageError::invalid("webp-decode"))?;
            (img, dropped)
        }
        SniffedFormat::Png | SniffedFormat::Jpeg => {
            if format == SniffedFormat::Jpeg && !jpeg_reaches_eoi(bytes) {
                // §4.1 strict structural mode: the JPEG marker walk must
                // reach EOI. zune-jpeg PADS missing scan data instead of
                // erroring (grok image_validate.rs documents this), so a
                // truncated JPEG could otherwise pass the full decode.
                return Err(ImageError::invalid("jpeg-truncated"));
            }
            let image_format = match format {
                SniffedFormat::Png => image::ImageFormat::Png,
                _ => image::ImageFormat::Jpeg,
            };
            let img = image::ImageReader::with_format(Cursor::new(bytes), image_format)
                .decode()
                .map_err(|_| ImageError::invalid("decode"))?;
            (img, false)
        }
    };

    // §4.5: EXIF orientation is APPLIED to the pixels before re-encode.
    let mut decoded = decoded;
    if format == SniffedFormat::Jpeg
        && let Some(exif_chunk) = jpeg_exif_chunk(bytes)
        && let Some(orientation) = image::metadata::Orientation::from_exif_chunk(&exif_chunk)
    {
        decoded.apply_orientation(orientation);
    }

    // §4.5: downscale to the edge/area normalization targets (Triangle).
    let (w, h) = (decoded.width(), decoded.height());
    let (tw, th) = downscale_target(w, h);
    if (tw, th) != (w, h) {
        decoded = decoded.resize(tw, th, image::imageops::FilterType::Triangle);
    }
    let (fw, fh) = (decoded.width(), decoded.height());

    // §4.5: re-encode targets — PNG when the source has alpha or was GIF,
    // the JPEG ladder otherwise. The encoders receive ONLY the freshly
    // allocated pixel buffer: EXIF/XMP/ICC/IPTC/COM/textual chunks have no
    // path to the output (the strip is structural).
    let has_alpha = decoded.color().has_alpha();
    let (out_bytes, wire_mime) = if has_alpha || format == SniffedFormat::Gif {
        let mut buf = Vec::new();
        encode_png(&decoded, &mut buf)?;
        if buf.len() as u64 > MAX_IMAGE_RAW_PAYLOAD_BYTES {
            return Err(ImageError::too_large("payload-png"));
        }
        (buf, WIRE_MIME_PNG)
    } else {
        let mut accepted = None;
        for quality in JPEG_QUALITY_STEPS {
            let mut buf = Vec::new();
            encode_jpeg(&decoded, &mut buf, quality)?;
            if buf.len() as u64 <= MAX_IMAGE_RAW_PAYLOAD_BYTES {
                accepted = Some(buf);
                break;
            }
        }
        match accepted {
            Some(buf) => (buf, WIRE_MIME_JPEG),
            None => return Err(ImageError::too_large("payload-ladder-floor")),
        }
    };

    let digest = hex_sha256(&out_bytes);
    Ok(LoadedImage {
        bytes: out_bytes,
        wire_mime: wire_mime.to_string(),
        sniffed_mime: format.mime().to_string(),
        digest,
        orig_path: None,
        placeholder: None,
        width: fw,
        height: fh,
        frames_dropped,
    })
}

/// §4.1 strict JPEG structure: the stream must reach EOI (0xFF 0xD9).
/// Entropy-coded data byte-stuffs 0xFF as 0xFF00, so a literal FF D9 pair
/// can only be the EOI marker (restart markers are FF D0–D7).
fn jpeg_reaches_eoi(bytes: &[u8]) -> bool {
    bytes.windows(2).any(|w| w == [0xFF, 0xD9])
}

/// Extracts the raw EXIF TIFF chunk (the APP1 payload after the
/// `Exif\0\0` prefix) from a JPEG byte stream, marker-walk only.
fn jpeg_exif_chunk(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut pos = 2usize;
    while pos + 4 <= bytes.len() {
        if bytes[pos] != 0xFF {
            return None; // not at a marker — entropy data, no EXIF
        }
        let marker = bytes[pos + 1];
        // Standalone markers carry no length.
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            pos += 2;
            continue;
        }
        let len = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
        if len < 2 || pos + 2 + len > bytes.len() {
            return None;
        }
        let payload = &bytes[pos + 4..pos + 2 + len];
        if marker == 0xE1 && payload.starts_with(b"Exif\0\0") {
            return Some(payload[6..].to_vec());
        }
        pos += 2 + len;
    }
    None
}

/// Fit-within scale for the §4.2 normalization targets (edge AND area),
/// computed in f64 (dims ≤ 65,535 are exact) and floored so the result
/// never exceeds either cap.
fn downscale_target(w: u32, h: u32) -> (u32, u32) {
    let wf = f64::from(w);
    let hf = f64::from(h);
    let edge = f64::from(MAX_IMAGE_DIMENSION) / wf.max(hf);
    let area = (MAX_IMAGE_PIXELS as f64 / (wf * hf)).sqrt();
    let scale = edge.min(area).min(1.0);
    if scale >= 1.0 {
        return (w, h);
    }
    let nw = ((wf * scale).floor() as u32).max(1);
    let nh = ((hf * scale).floor() as u32).max(1);
    (nw, nh)
}

fn encode_png(img: &image::DynamicImage, out: &mut Vec<u8>) -> Result<(), ImageError> {
    use image::ImageEncoder as _;
    image::codecs::png::PngEncoder::new(out)
        .write_image(
            img.as_bytes(),
            img.width(),
            img.height(),
            img.color().into(),
        )
        .map_err(|_| ImageError::invalid("png-encode"))
}

fn encode_jpeg(
    img: &image::DynamicImage,
    out: &mut Vec<u8>,
    quality: u8,
) -> Result<(), ImageError> {
    use image::ImageEncoder as _;
    let rgb = img.to_rgb8();
    image::codecs::jpeg::JpegEncoder::new_with_quality(out, quality)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|_| ImageError::invalid("jpeg-encode"))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

// ───────────────────────────── §12 test battery ─────────────────────────
// Synthetic fixtures ONLY — generated byte streams, no real photos. The
// generator lives here; small deterministic fixtures are materialized under
// `crates/nano-tools/fixtures/images/` (and committed) so reviewers can
// inspect the exact bytes; large/oversize cases stay in memory.
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── fixture generator ────────────────────────────────────────────────

    fn fixture_dir() -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/images");
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

    /// Writes the generated bytes to the named fixture when missing
    /// (committed on first generation; deterministic thereafter) and
    /// returns the bytes. When the file exists its content MUST match the
    /// generator — a stale fixture on disk is a loud failure, never a
    /// silent reuse.
    fn materialize(name: &str, bytes: &[u8]) -> Vec<u8> {
        let path = fixture_dir().join(name);
        if path.exists() {
            let on_disk = std::fs::read(&path).expect("read fixture");
            assert_eq!(
                on_disk, bytes,
                "fixture {name} on disk diverged from the generator — regenerate it"
            );
        } else {
            std::fs::write(&path, bytes).expect("write fixture");
        }
        bytes.to_vec()
    }

    /// xorshift64* — deterministic noise pixels (near-incompressible).
    struct Noise(u64);
    impl Noise {
        fn next(&mut self) -> u8 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8
        }
    }

    fn rgb_image(w: u32, h: u32, noise: bool) -> image::RgbImage {
        let mut rng = Noise(0x9E37_79B9_7F4A_7C15);
        image::RgbImage::from_fn(w, h, |x, y| {
            if noise {
                image::Rgb([rng.next(), rng.next(), rng.next()])
            } else {
                image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 233) as u8])
            }
        })
    }

    fn encode_png_bytes(img: &image::DynamicImage) -> Vec<u8> {
        use image::ImageEncoder as _;
        let mut buf = Vec::new();
        image::codecs::png::PngEncoder::new(&mut buf)
            .write_image(
                img.as_bytes(),
                img.width(),
                img.height(),
                img.color().into(),
            )
            .expect("png encode");
        buf
    }

    fn encode_jpeg_bytes(img: &image::DynamicImage, quality: u8) -> Vec<u8> {
        use image::ImageEncoder as _;
        let rgb = img.to_rgb8();
        let mut buf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .expect("jpeg encode");
        buf
    }

    /// Table-driven CRC-32 (IEEE) for hand-built PNG fixtures.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
            }
        }
        !crc
    }

    fn png_chunk(chunk_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(chunk_type);
        out.extend_from_slice(data);
        let mut crc_input = chunk_type.to_vec();
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        out
    }

    /// A structurally valid PNG carrying arbitrary declared dimensions over
    /// a tiny body — the bomb-header fixture family.
    fn png_with_declared_dims(w: u32, h: u32) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
        out.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
        // A minimal (invalid-for-real-dims but present) IDAT + IEND: the
        // header probe must reject before either is ever decoded.
        out.extend_from_slice(&png_chunk(
            b"IDAT",
            &[0x78, 0x9c, 0x00, 0x00, 0x00, 0xff, 0xff],
        ));
        out.extend_from_slice(&png_chunk(b"IEND", &[]));
        out
    }

    /// Inserts a chunk right after IHDR (offset 8 + 25) in a PNG.
    fn png_insert_after_ihdr(png: &[u8], chunk: &[u8]) -> Vec<u8> {
        let at = 8 + 25;
        let mut out = png[..at].to_vec();
        out.extend_from_slice(chunk);
        out.extend_from_slice(&png[at..]);
        out
    }

    /// Hand-built EXIF TIFF payload (little-endian): orientation tag 0x0112
    /// plus a GPS IFD carrying a metadata canary.
    fn exif_tiff_payload(orientation: u16) -> Vec<u8> {
        let canary = b"NANOCANARYGPS\0";
        let gps_ifd_offset = 8 + 2 + 2 * 12 + 4; // hdr + count + 2 entries + next
        let canary_offset = gps_ifd_offset + 2 + 12 + 4;
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II\x2a\x00"); // little-endian, magic 42
        tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        // IFD0: two entries.
        tiff.extend_from_slice(&2u16.to_le_bytes());
        // tag 0x0112 (orientation), SHORT(3), count 1, value inline.
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&orientation.to_le_bytes());
        tiff.extend_from_slice(&0u16.to_le_bytes());
        // tag 0x8825 (GPSInfo pointer), LONG(4), count 1, value = offset.
        tiff.extend_from_slice(&0x8825u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&(gps_ifd_offset as u32).to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes()); // next IFD = none
        // GPS IFD: one entry — GPSLatitudeRef(0x0001), ASCII(2), canary.
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0001u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&(canary.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&(canary_offset as u32).to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes()); // next IFD = none
        tiff.extend_from_slice(canary);
        tiff
    }

    /// Splices an APP1 (Exif) segment right after the SOI marker.
    fn jpeg_insert_app1(jpeg: &[u8], exif_tiff: &[u8]) -> Vec<u8> {
        assert_eq!(&jpeg[0..2], &[0xFF, 0xD8], "fixture starts with SOI");
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(exif_tiff);
        let len = (payload.len() + 2) as u16;
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    /// Splices a COM segment carrying `text` right after the SOI marker.
    fn jpeg_insert_com(jpeg: &[u8], text: &[u8]) -> Vec<u8> {
        assert_eq!(&jpeg[0..2], &[0xFF, 0xD8], "fixture starts with SOI");
        let len = (text.len() + 2) as u16;
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xFE]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(text);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    /// Wraps a still lossless WebP as a two-frame animated VP8X container
    /// (frame 2 carries a different color so first-frame-only is provable).
    fn animated_webp(frame1: &[u8], frame2: &[u8], w: u32, h: u32) -> Vec<u8> {
        fn vp8l_payload(webp: &[u8]) -> Vec<u8> {
            // Extract the VP8L chunk payload from a still (header skipped).
            assert_eq!(&webp[0..4], b"RIFF");
            let mut pos = 12usize;
            while pos + 8 <= webp.len() {
                let id = &webp[pos..pos + 4];
                let size = u32::from_le_bytes(webp[pos + 4..pos + 8].try_into().unwrap()) as usize;
                if id == b"VP8L" {
                    return webp[pos + 8..pos + 8 + size].to_vec();
                }
                pos += 8 + size + (size % 2);
            }
            panic!("still webp carries a VP8L chunk");
        }
        fn anmf(frame_payload: &[u8], w: u32, h: u32) -> Vec<u8> {
            // Frame data = the VP8L chunk (fourcc + size + payload). The
            // inner chunk's word-alignment pad is INSIDE the ANMF and
            // counted in its size (image-webp requires
            // chunk_size_rounded + 24 <= anmf_size).
            let mut frame_data = b"VP8L".to_vec();
            frame_data.extend_from_slice(&(frame_payload.len() as u32).to_le_bytes());
            frame_data.extend_from_slice(frame_payload);
            if frame_payload.len() % 2 != 0 {
                frame_data.push(0);
            }
            let mut header = Vec::new();
            header.extend_from_slice(&[0, 0, 0]); // x/2 (24-bit)
            header.extend_from_slice(&[0, 0, 0]); // y/2
            header.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
            header.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
            header.extend_from_slice(&100u32.to_le_bytes()[..3]); // duration ms
            header.push(0); // flags
            let mut out = b"ANMF".to_vec();
            out.extend_from_slice(&((header.len() + frame_data.len()) as u32).to_le_bytes());
            out.extend_from_slice(&header);
            out.extend_from_slice(&frame_data);
            if out.len() % 2 != 0 {
                out.push(0); // RIFF word alignment (pad NOT counted in size)
            }
            out
        }
        let mut payload = b"WEBP".to_vec();
        // VP8X: animation flag (0x02), canvas dims.
        let mut vp8x = vec![0x02, 0, 0, 0];
        vp8x.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
        vp8x.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
        payload.extend_from_slice(b"VP8X");
        payload.extend_from_slice(&(vp8x.len() as u32).to_le_bytes());
        payload.extend_from_slice(&vp8x);
        // ANIM: background BGRA + loop count 0 (forever).
        payload.extend_from_slice(b"ANIM");
        payload.extend_from_slice(&6u32.to_le_bytes());
        payload.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        payload.extend_from_slice(&anmf(&vp8l_payload(frame1), w, h));
        payload.extend_from_slice(&anmf(&vp8l_payload(frame2), w, h));
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    fn still_webp(dominant: u8) -> Vec<u8> {
        use image::ImageEncoder as _;
        // 32×32 noise — the VP8L payload must clear image-webp's ANMF
        // minimum (anmf_size >= 32) when wrapped as an animation frame.
        let mut rng = Noise(0xDEAD_BEEF_0000_0000 | u64::from(dominant));
        let img = image::RgbImage::from_fn(32, 32, |_, _| match dominant {
            0 => image::Rgb([rng.next(), 20, 20]),
            _ => image::Rgb([20, 20, rng.next()]),
        });
        let mut buf = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut buf)
            .write_image(img.as_raw(), 32, 32, image::ExtendedColorType::Rgb8)
            .expect("webp encode");
        buf
    }

    fn gif_frames(count: usize) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(1, 1, |_, _| image::Rgba([7, 8, 9, 255]));
        let frame = image::Frame::new(img);
        let mut buf = Vec::new();
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut buf);
        encoder
            .encode_frames(vec![frame; count])
            .expect("gif encode");
        drop(encoder);
        buf
    }

    // ── structural output enumeration (§4.5/§12: the PRIMARY oracle — a
    // byte-substring scan alone is insufficient proof) ─────────────────────

    fn png_chunk_types(png: &[u8]) -> Vec<[u8; 4]> {
        assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n", "output is a PNG");
        let mut types = Vec::new();
        let mut pos = 8usize;
        while pos + 8 <= png.len() {
            let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
            let ctype: [u8; 4] = png[pos + 4..pos + 8].try_into().unwrap();
            types.push(ctype);
            pos += 8 + len + 4;
            if &ctype == b"IEND" {
                break;
            }
        }
        types
    }

    /// JPEG segment markers BEFORE the SOS (metadata segments live pre-scan).
    fn jpeg_segment_markers(jpeg: &[u8]) -> Vec<u8> {
        assert_eq!(&jpeg[0..2], &[0xFF, 0xD8], "output is a JPEG");
        let mut markers = Vec::new();
        let mut pos = 2usize;
        while pos + 4 <= jpeg.len() {
            assert_eq!(jpeg[pos], 0xFF, "marker walk stays on markers");
            let marker = jpeg[pos + 1];
            if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
                if marker == 0xD9 {
                    markers.push(marker);
                    break;
                }
                pos += 2;
                continue;
            }
            markers.push(marker);
            if marker == 0xDA {
                break; // start of scan: entropy data follows
            }
            let len = u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
            pos += 2 + len;
        }
        markers
    }

    // ── loader battery ───────────────────────────────────────────────────

    #[tokio::test]
    async fn valid_four_format_round_trip() {
        let png = materialize(
            "valid.png",
            &encode_png_bytes(&image::DynamicImage::ImageRgb8(rgb_image(40, 30, false))),
        );
        let jpeg = materialize(
            "valid.jpeg",
            &encode_jpeg_bytes(
                &image::DynamicImage::ImageRgb8(rgb_image(40, 30, false)),
                85,
            ),
        );
        let gif = materialize("valid.gif", &gif_frames(2));
        let webp = materialize("valid.webp", &still_webp(0));
        for (bytes, claim, sniffed, dims) in [
            (png, "image/png", "image/png", (40, 30)),
            (jpeg, "image/jpeg", "image/jpeg", (40, 30)),
            (gif, "image/gif", "image/gif", (1, 1)),
            (webp, "image/webp", "image/webp", (32, 32)),
        ] {
            let loaded = load_image(&bytes, Some(claim))
                .await
                .expect("valid image loads");
            assert_eq!(loaded.sniffed_mime, sniffed);
            // Wire emission is png/jpeg only (§4.5).
            assert!(matches!(
                loaded.wire_mime.as_str(),
                "image/png" | "image/jpeg"
            ));
            assert!(loaded.bytes.len() as u64 <= MAX_IMAGE_RAW_PAYLOAD_BYTES);
            // The digest is over the RE-ENCODED bytes (§4.6).
            assert_eq!(loaded.digest, hex_sha256(&loaded.bytes));
            assert_eq!((loaded.width, loaded.height), dims);
            assert!(loaded.receipt_line().contains("sniffed="));
        }
    }

    #[tokio::test]
    async fn corrupt_crc_png_is_typed_invalid() {
        let mut png = encode_png_bytes(&image::DynamicImage::ImageRgb8(rgb_image(16, 16, false)));
        let at = png
            .windows(4)
            .position(|w| w == b"IDAT")
            .expect("png carries IDAT");
        png[at + 6] ^= 0xFF; // flip inside IDAT data → CRC mismatch
        let bytes = materialize("corrupt-crc.png", &png);
        let err = load_image(&bytes, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageInvalid, "{err}");
    }

    #[tokio::test]
    async fn truncated_jpeg_is_typed_invalid() {
        let jpeg = encode_jpeg_bytes(
            &image::DynamicImage::ImageRgb8(rgb_image(32, 32, false)),
            85,
        );
        // Truncate right after the SOF0 segment: the header probe still
        // reads dimensions, but the entropy data + tables are GONE, so the
        // full decode must fail. (Tail-only truncation can still decode —
        // zune-jpeg tolerates a missing EOI once all MCUs are present.)
        let sof = jpeg
            .windows(2)
            .position(|w| w == [0xFF, 0xC0])
            .expect("jpeg carries SOF0");
        let sof_len = u16::from_be_bytes([jpeg[sof + 2], jpeg[sof + 3]]) as usize;
        let cut = jpeg[..sof + 2 + sof_len].to_vec();
        let bytes = materialize("truncated.jpeg", &cut);
        let err = load_image(&bytes, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageInvalid, "{err}");
    }

    #[tokio::test]
    async fn tail_truncated_but_decodable_jpeg_is_still_rejected() {
        // zune-jpeg PADS missing scan data, so a tail-truncated JPEG can
        // pass a raw pixel decode (grok image_validate.rs documents this).
        // §4.1 strict structural mode rejects it via the EOI walk.
        let jpeg = encode_jpeg_bytes(
            &image::DynamicImage::ImageRgb8(rgb_image(32, 32, false)),
            85,
        );
        let cut = jpeg[..jpeg.len() - 64].to_vec(); // EOI gone, entropy almost whole
        let bytes = materialize("tail-truncated.jpeg", &cut);
        let err = load_image(&bytes, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageInvalid, "{err}");
        assert_eq!(err.detail, "jpeg-truncated");
    }

    #[tokio::test]
    async fn forged_extension_text_bytes_are_typed_invalid() {
        let bytes = materialize("forged-extension.png.txt", b"this is not an image at all");
        // The byte loader never sees the extension (pre-filter only, §3.3);
        // text bytes fail the magic-byte sniff.
        let err = load_image(&bytes, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageInvalid, "{err}");
    }

    #[tokio::test]
    async fn mime_claim_mismatch_is_typed_invalid() {
        let png = encode_png_bytes(&image::DynamicImage::ImageRgb8(rgb_image(8, 8, false)));
        let err = load_image(&png, Some("image/jpeg")).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageInvalid);
        assert_eq!(err.detail, "mime-claim-mismatch");
        // A claim outside the closed set is rejected too — never re-labeled.
        let err = load_image(&png, Some("image/bmp")).await.unwrap_err();
        assert_eq!(err.detail, "mime-claim-unknown");
        // The CORRECT claim passes.
        load_image(&png, Some("image/png"))
            .await
            .expect("matching claim");
    }

    #[tokio::test]
    async fn svg_and_bmp_are_typed_unsupported() {
        let svg = materialize(
            "vector.svg.bin",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>",
        );
        let err = load_image(&svg, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageUnsupportedFormat, "{err}");
        let xml_svg = b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let err = load_image(xml_svg, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageUnsupportedFormat, "{err}");
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0u8; 68]);
        let bytes = materialize("bitmap.bmp.bin", &bmp);
        let err = load_image(&bytes, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageUnsupportedFormat, "{err}");
    }

    #[tokio::test]
    async fn oversize_file_is_too_large_with_zero_decode() {
        let before = TOTAL_DECODES.load(Ordering::SeqCst);
        let bytes = vec![0u8; (MAX_IMAGE_FILE_BYTES + 1) as usize];
        let err = load_image(&bytes, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageTooLarge);
        assert_eq!(err.detail, "file-bytes");
        assert_eq!(
            TOTAL_DECODES.load(Ordering::SeqCst),
            before,
            "oversize rejection runs ZERO decodes"
        );
    }

    #[tokio::test]
    async fn bomb_header_rejected_before_allocation() {
        // 40,000 × 40,000 = 1.6 Gpx declared over a ~100 B file.
        let bomb = materialize("bomb-header.png", &png_with_declared_dims(40_000, 40_000));
        let before = TOTAL_DECODES.load(Ordering::SeqCst);
        let err = load_image(&bomb, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageTooLarge);
        assert_eq!(err.detail, "decode-pixels");
        assert_eq!(
            TOTAL_DECODES.load(Ordering::SeqCst),
            before,
            "the header probe rejects BEFORE the decoder allocates"
        );
    }

    #[tokio::test]
    async fn oversized_dimensions_are_typed_errors_via_checked_arithmetic() {
        // Each dimension > 65,535 → rejection; note 65,536² = 2³² OVERFLOWS
        // u32, so the checked_mul arm fires first — naive arithmetic would
        // wrap to 0 and pass a pixel check. That is the whole point of the
        // fixture.
        let fixture = materialize("dim-65536.png", &png_with_declared_dims(65_536, 65_536));
        let err = load_image(&fixture, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageTooLarge);
        assert_eq!(err.detail, "dimension-overflow");
        assert!(65_536u32.checked_mul(65_536).is_none());
        // One over-bound dimension with a small partner reaches the
        // per-dimension sanity bound directly.
        let wide = png_with_declared_dims(65_536, 1);
        let err = load_image(&wide, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageTooLarge);
        assert_eq!(err.detail, "dimension");
        // Zero dimensions are invalid, not a division-by-zero.
        let zero = png_with_declared_dims(0, 100);
        let err = load_image(&zero, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageInvalid);
    }

    #[tokio::test]
    async fn over_edge_image_is_downscaled_not_rejected() {
        // Solid color: the fixture's DIMENSIONS are what matter, and a flat
        // 3000×3000 stays tiny on disk (committed fixture).
        let img = image::RgbImage::from_fn(3000, 3000, |_, _| image::Rgb([12, 200, 90]));
        let png = encode_png_bytes(&image::DynamicImage::ImageRgb8(img));
        let bytes = materialize("downscale-3000.png", &png);
        let loaded = load_image(&bytes, None)
            .await
            .expect("3000x3000 downscales");
        assert!(loaded.width <= MAX_IMAGE_DIMENSION && loaded.height <= MAX_IMAGE_DIMENSION);
        assert!(
            u64::from(loaded.width) * u64::from(loaded.height) <= MAX_IMAGE_PIXELS,
            "{}x{} exceeds the normalization target",
            loaded.width,
            loaded.height
        );
        assert_eq!(loaded.width, 1024); // 3000² → 1024×1024 (area dominates)
        assert_eq!(loaded.height, 1024);
    }

    #[tokio::test]
    async fn noisy_png_lands_under_payload_cap_via_ladder() {
        // ~900 KiB of near-incompressible RGB noise, no alpha → JPEG ladder.
        let png = encode_png_bytes(&image::DynamicImage::ImageRgb8(rgb_image(600, 500, true)));
        assert!(
            png.len() > 700 * 1024,
            "fixture is genuinely large: {}",
            png.len()
        );
        let bytes = materialize("noisy-900k.png", &png);
        let loaded = load_image(&bytes, None)
            .await
            .expect("ladder lands under the cap");
        assert_eq!(loaded.wire_mime, "image/jpeg");
        assert!(loaded.bytes.len() as u64 <= MAX_IMAGE_RAW_PAYLOAD_BYTES);
        // Base64 expansion fits the wire cap (checked arithmetic).
        let b64_len = (loaded.bytes.len() as u64).div_ceil(3) * 4;
        assert!(b64_len <= MAX_IMAGE_PAYLOAD_BYTES);
    }

    #[tokio::test]
    async fn alpha_source_reencodes_as_png_and_preserves_alpha() {
        let img = image::RgbaImage::from_fn(16, 16, |x, _| {
            image::Rgba([x as u8, 0, 255, if x < 8 { 128 } else { 255 }])
        });
        let png = encode_png_bytes(&image::DynamicImage::ImageRgba8(img));
        let loaded = load_image(&png, None).await.expect("alpha png loads");
        assert_eq!(loaded.wire_mime, "image/png", "alpha never crosses as JPEG");
    }

    #[tokio::test]
    async fn exif_orientation_applied_and_metadata_structurally_absent() {
        // 2×1 JPEG, left red / right blue, EXIF orientation 6 (Rotate90),
        // plus a GPS IFD canary.
        let img = image::RgbImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });
        let jpeg = encode_jpeg_bytes(&image::DynamicImage::ImageRgb8(img), 95);
        let exif = jpeg_insert_app1(&jpeg, &exif_tiff_payload(6));
        let bytes = materialize("exif-gps-orient6.jpeg", &exif);
        let loaded = load_image(&bytes, None).await.expect("exif jpeg loads");
        assert_eq!(
            (loaded.width, loaded.height),
            (1, 2),
            "orientation 6 rotated the 2x1 source to 1x2"
        );
        // Structural proof (§4.5): enumerate the output's JPEG segments —
        // no APP1 (0xE1), no APP13 (0xED), no COM (0xFE) can exist.
        let markers = jpeg_segment_markers(&loaded.bytes);
        assert!(!markers.contains(&0xE1), "no APP1/EXIF: {markers:02x?}");
        assert!(!markers.contains(&0xED), "no APP13/IPTC: {markers:02x?}");
        assert!(!markers.contains(&0xFE), "no COM: {markers:02x?}");
        // Secondary (never primary) byte check: the GPS canary is gone.
        let window = b"NANOCANARYGPS";
        assert!(
            !loaded.bytes.windows(window.len()).any(|w| w == window),
            "metadata canary structurally absent from the wire bytes"
        );
    }

    #[tokio::test]
    async fn png_text_chunk_canary_does_not_survive_reencode() {
        // Alpha-carrying source → the re-encode target is PNG (§4.5), so
        // the output's chunk list is enumerable per §12.
        let img = image::RgbaImage::from_fn(16, 16, |x, _| image::Rgba([x as u8, 1, 2, 200]));
        let png = encode_png_bytes(&image::DynamicImage::ImageRgba8(img));
        let text = png_insert_after_ihdr(&png, &png_chunk(b"tEXt", b"gps\0NANOCANARYTEXT"));
        let bytes = materialize("png-text-canary.png", &text);
        let loaded = load_image(&bytes, None).await.expect("tEXt png loads");
        assert_eq!(loaded.wire_mime, "image/png");
        const ALLOWED: [[u8; 4]; 5] = [*b"IHDR", *b"PLTE", *b"tRNS", *b"IDAT", *b"IEND"];
        let chunks = png_chunk_types(&loaded.bytes);
        for ctype in &chunks {
            assert!(
                ALLOWED.contains(ctype),
                "output chunk set stays minimal: {chunks:?}"
            );
        }
        assert!(!chunks.iter().any(|c| c == b"tEXt"));
        let window = b"NANOCANARYTEXT";
        assert!(!loaded.bytes.windows(window.len()).any(|w| w == window));
    }

    #[tokio::test]
    async fn jpeg_com_canary_does_not_survive_reencode() {
        let jpeg = encode_jpeg_bytes(
            &image::DynamicImage::ImageRgb8(rgb_image(16, 16, false)),
            90,
        );
        let with_com = jpeg_insert_com(&jpeg, b"NANOCANARYCOM");
        let bytes = materialize("jpeg-com-canary.jpeg", &with_com);
        let loaded = load_image(&bytes, None).await.expect("COM jpeg loads");
        let markers = jpeg_segment_markers(&loaded.bytes);
        assert!(!markers.contains(&0xFE), "no COM in output: {markers:02x?}");
        let window = b"NANOCANARYCOM";
        assert!(!loaded.bytes.windows(window.len()).any(|w| w == window));
    }

    #[tokio::test]
    async fn polyglot_trailing_payloads_never_reach_the_output() {
        // PNG (alpha-bearing → PNG output) + appended ZIP container.
        let img = image::RgbaImage::from_fn(16, 16, |x, _| image::Rgba([x as u8, 1, 2, 255]));
        let png = encode_png_bytes(&image::DynamicImage::ImageRgba8(img));
        let mut zip_poly = png.clone();
        zip_poly.extend_from_slice(b"PK\x03\x04");
        zip_poly.extend_from_slice(&[0u8; 32]);
        zip_poly.extend_from_slice(b"PK\x05\x06"); // EOCD
        let bytes = materialize("polyglot-png-zip.png", &zip_poly);
        let loaded = load_image(&bytes, None)
            .await
            .expect("trailing ZIP tolerated");
        let chunks = png_chunk_types(&loaded.bytes);
        assert_eq!(chunks.last(), Some(b"IEND"), "fresh PNG ends at IEND");
        assert!(!loaded.bytes.windows(2).any(|w| w == b"PK"));

        // JPEG + trailing HTML.
        let jpeg = encode_jpeg_bytes(
            &image::DynamicImage::ImageRgb8(rgb_image(16, 16, false)),
            90,
        );
        let mut html_poly = jpeg.clone();
        html_poly.extend_from_slice(b"<html><script>alert(1)</script></html>");
        let bytes = materialize("polyglot-jpeg-html.jpeg", &html_poly);
        let loaded = load_image(&bytes, None)
            .await
            .expect("trailing HTML tolerated");
        assert!(
            loaded.bytes.ends_with(&[0xFF, 0xD9]),
            "fresh JPEG ends at EOI"
        );
        assert!(!loaded.bytes.windows(6).any(|w| w == b"<html>"));

        // GIF + trailing JS.
        let gif = gif_frames(1);
        let mut js_poly = gif.clone();
        js_poly.extend_from_slice(b";alert(1)//");
        let bytes = materialize("polyglot-gif-js.gif", &js_poly);
        let loaded = load_image(&bytes, None)
            .await
            .expect("trailing JS tolerated");
        assert_eq!(loaded.wire_mime, "image/png", "GIF re-encodes to PNG");
        assert!(!loaded.bytes.windows(5).any(|w| w == b"alert"));
    }

    #[tokio::test]
    async fn thousand_frame_gif_decodes_first_frame_only() {
        let gif = materialize("anim-1000f.gif", &gif_frames(1000));
        let before = TOTAL_DECODES.load(Ordering::SeqCst);
        let loaded = load_image(&gif, None).await.expect("gif loads");
        assert!(loaded.frames_dropped, "999 frames were dropped");
        assert_eq!(loaded.wire_mime, "image/png");
        assert_eq!((loaded.width, loaded.height), (1, 1));
        // ONE decode body ran (no per-frame decode explosion).
        assert_eq!(TOTAL_DECODES.load(Ordering::SeqCst), before + 1);
        assert!(loaded.receipt_line().contains("frames: N -> 1"));
    }

    #[tokio::test]
    async fn animated_webp_decodes_first_frame_only() {
        let frame1 = still_webp(0); // red-dominant noise
        let frame2 = still_webp(1); // blue-dominant noise
        let anim = materialize("anim.webp", &animated_webp(&frame1, &frame2, 32, 32));
        let loaded = load_image(&anim, None).await.expect("animated webp loads");
        assert!(loaded.frames_dropped, "ANIM container reported");
        // First frame was red-dominant — the second (blue) never rendered.
        let decoded = image::ImageReader::new(Cursor::new(&loaded.bytes))
            .with_guessed_format()
            .expect("output sniffs")
            .decode()
            .expect("output parses");
        let rgb = decoded.to_rgb8();
        let mut red = 0u64;
        let mut blue = 0u64;
        for px in rgb.pixels() {
            red += u64::from(px[0]);
            blue += u64::from(px[2]);
        }
        assert!(red > blue * 2, "frame 1 (red) won: r={red} b={blue}");
    }

    #[tokio::test]
    async fn decodes_are_sequential_process_wide() {
        let png = encode_png_bytes(&image::DynamicImage::ImageRgb8(rgb_image(900, 700, true)));
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let bytes = png.clone();
            set.spawn(async move { load_image(&bytes, None).await.expect("load") });
        }
        while set.join_next().await.is_some() {}
        assert_eq!(
            MAX_OBSERVED_ACTIVE_DECODES.load(Ordering::SeqCst),
            1,
            "the 1-permit semaphore serializes every decode (§4.2)"
        );
    }

    #[tokio::test]
    async fn poisoned_decoder_panics_are_contained_and_typed() {
        // Pad to a length no other fixture uses (trailing bytes past IEND
        // are tolerated, §4.1) so the length-targeted poison hits exactly
        // this test's decode, never a parallel test's.
        let mut png = encode_png_bytes(&image::DynamicImage::ImageRgb8(rgb_image(8, 8, false)));
        png.resize(40_001, 0);
        POISON_DECODE_LEN.store(png.len(), Ordering::SeqCst);
        let err = load_image(&png, None).await.unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageInvalid, "{err}");
        // Host alive, permit released, counter balanced: the next load works.
        assert_eq!(ACTIVE_DECODES.load(Ordering::SeqCst), 0);
        load_image(&png, None)
            .await
            .expect("loader survives a contained panic");
    }

    #[tokio::test]
    async fn join_error_mapping_panic_and_cancelled() {
        let panicked = tokio::task::spawn_blocking(|| panic!("boom"));
        let err = panicked.await.unwrap_err();
        assert!(err.is_panic());
        assert_eq!(map_join_error(err).kind, NanoErrorKind::ImageInvalid);

        let cancelled = tokio::task::spawn(async {
            std::future::pending::<()>().await;
        });
        cancelled.abort();
        let err = cancelled.await.unwrap_err();
        assert!(err.is_cancelled());
        assert_eq!(map_join_error(err).kind, NanoErrorKind::UserCancelled);
    }

    #[test]
    fn prompt_limits_count_and_aggregate_are_saturating() {
        // 16 accepted, the 17th → ImageTooMany.
        assert!(check_prompt_limits([1u64; 16]).is_ok());
        let err = check_prompt_limits([1u64; 17]).unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageTooMany);
        assert_eq!(err.detail, "count");
        // Aggregate: 50 MiB exactly is fine; +1 B is not.
        assert!(check_prompt_limits([MAX_PROMPT_IMAGE_AGGREGATE_BYTES]).is_ok());
        let err = check_prompt_limits([MAX_PROMPT_IMAGE_AGGREGATE_BYTES, 1]).unwrap_err();
        assert_eq!(err.detail, "aggregate");
        // Saturating: absurd sizes never wrap.
        let err = check_prompt_limits([u64::MAX, u64::MAX]).unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageTooMany);
    }

    #[tokio::test]
    async fn read_image_file_capped_enforces_the_ceiling_before_decode() {
        let dir = std::env::temp_dir().join(format!("nano-image-capped-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.png");
        std::fs::write(&small, b"\x89PNG\r\n\x1a\nrest").unwrap();
        assert_eq!(read_image_file_capped(&small).unwrap().len(), 12);
        let big = dir.join("big.bin");
        std::fs::write(&big, vec![0u8; (MAX_IMAGE_FILE_BYTES + 1) as usize]).unwrap();
        let err = read_image_file_capped(&big).unwrap_err();
        assert_eq!(err.kind, NanoErrorKind::ImageTooLarge);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── workspace invariants (§4.3/D11, §4.7/D8) ─────────────────────────

    #[test]
    fn workspace_profile_never_sets_panic_abort() {
        // catch_unwind is INERT under panic="abort"; if any profile ever
        // sets it, the §4.3 panic containment silently stops working — so
        // this test fails CI loudly instead.
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let manifest =
            std::fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest");
        assert!(
            !manifest.replace(' ', "").contains("panic=\"abort\""),
            "workspace profiles must keep panic = \"unwind\""
        );
    }

    #[test]
    fn image_crate_feature_lock() {
        // D8: the decoder allowlist is LINK-LEVEL. Assert both manifests:
        // the workspace table carries the pinned, default-features-off
        // allowlist, and nano-tools adds NO local features.
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let ws = std::fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest");
        let normalized = ws.replace([' ', '\n', '\t'], "");
        assert!(
            normalized.contains("image={version=\"=0.25.10\",default-features=false"),
            "image is pinned exact with default-features off"
        );
        for allowed in ["\"png\"", "\"jpeg\"", "\"gif\"", "\"webp\""] {
            assert!(normalized.contains(allowed), "{allowed} in the allowlist");
        }
        // No decoder outside the closed set may appear in the image entry.
        let entry_start = normalized
            .find("image={version=")
            .expect("workspace image entry");
        let entry_end = normalized[entry_start..].find("]}").expect("entry closes") + entry_start;
        let entry = &normalized[entry_start..entry_end];
        for forbidden in [
            "\"bmp\"",
            "\"tiff\"",
            "\"avif\"",
            "\"exr\"",
            "\"qoi\"",
            "\"tga\"",
            "\"pnm\"",
            "\"ico\"",
            "\"hdr\"",
            "\"dds\"",
            "\"ff\"",
            "\"rayon\"",
            "\"exif\"",
        ] {
            assert!(!entry.contains(forbidden), "{forbidden} must stay unlinked");
        }
        let local =
            std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
                .expect("crate manifest");
        assert!(
            local.contains("image.workspace = true"),
            "nano-tools consumes the workspace-pinned image only"
        );
    }
}
