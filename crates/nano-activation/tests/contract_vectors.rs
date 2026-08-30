use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use nano_activation::{
    RejectReason, verify_activation_frame, verify_admin_request, verify_control, verify_receipt,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

#[derive(Deserialize)]
struct Manifest {
    positive_count: usize,
    negative_count: usize,
    subject_families: Vec<String>,
    files: Vec<ManifestFile>,
}

#[derive(Deserialize)]
struct ManifestFile {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
struct Positive {
    rfc8032: Rfc8032,
    activation: ActivationVector,
    receipt: ReceiptVector,
    control: ControlVector,
    admin: AdminVector,
}

#[derive(Deserialize)]
struct Rfc8032 {
    public_key_hex: String,
    message_hex: String,
    signature_hex: String,
}

#[derive(Deserialize)]
struct ActivationVector {
    public_key_hex: String,
    canonical_payload_sha256: String,
    raw_frame_utf8: String,
    expected_effective_authority: serde_json::Value,
}

#[derive(Deserialize)]
struct ReceiptVector {
    public_key_hex: String,
    raw_receipt_utf8: String,
}

#[derive(Deserialize)]
struct ControlVector {
    public_key_hex: String,
    raw_control_utf8: String,
}

#[derive(Deserialize)]
struct AdminVector {
    public_key_hex: String,
    raw_admin_utf8: String,
}

#[derive(Deserialize)]
struct Negative {
    zero_state: Vec<String>,
    cases: Vec<NegativeCase>,
}

#[derive(Deserialize)]
struct NegativeCase {
    name: String,
    #[serde(default)]
    raw_utf8: Option<String>,
    #[serde(default)]
    raw_base64: Option<String>,
    reason: RejectReason,
}

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/activation/vectors")
}

fn hex_array<const N: usize>(encoded: &str) -> [u8; N] {
    let bytes = decode_hex(encoded);
    bytes
        .try_into()
        .unwrap_or_else(|_| panic!("expected {N} bytes"))
}

fn decode_hex(encoded: &str) -> Vec<u8> {
    assert_eq!(encoded.len() % 2, 0, "fixture hex must have even length");
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).expect("valid fixture hex");
            let low = (pair[1] as char).to_digit(16).expect("valid fixture hex");
            ((high << 4) | low) as u8
        })
        .collect()
}

fn encode_hex(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[test]
fn manifest_is_complete_and_hash_bound() {
    let root = vectors();
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest.positive_count, 5);
    assert!(manifest.negative_count >= 20);
    assert_eq!(manifest.subject_families.len(), 8);
    assert_eq!(manifest.files.len(), 2);
    for entry in manifest.files {
        let bytes = fs::read(root.join(entry.path)).unwrap();
        assert_eq!(encode_hex(Sha256::digest(bytes)), entry.sha256);
    }
}

#[test]
fn published_and_fixture_signatures_verify() {
    let positive: Positive =
        serde_json::from_slice(&fs::read(vectors().join("positive.json")).unwrap()).unwrap();
    let rfc_key = VerifyingKey::from_bytes(&hex_array(&positive.rfc8032.public_key_hex)).unwrap();
    let rfc_signature = Signature::from_bytes(&hex_array(&positive.rfc8032.signature_hex));
    rfc_key
        .verify(&decode_hex(&positive.rfc8032.message_hex), &rfc_signature)
        .unwrap();

    let key = hex_array(&positive.activation.public_key_hex);
    let admitted =
        verify_activation_frame(positive.activation.raw_frame_utf8.as_bytes(), &key).unwrap();
    assert_eq!(admitted.principal_id(), "main");
    assert_eq!(admitted.project_id(), "project-018f");
    assert_eq!(
        admitted.canonical_payload_sha256(),
        positive.activation.canonical_payload_sha256
    );
    assert_eq!(
        positive.activation.expected_effective_authority["principal_id"],
        "main"
    );
    verify_receipt(
        positive.receipt.raw_receipt_utf8.as_bytes(),
        &hex_array(&positive.receipt.public_key_hex),
    )
    .unwrap();
    let control = verify_control(
        positive.control.raw_control_utf8.as_bytes(),
        &hex_array(&positive.control.public_key_hex),
    )
    .unwrap();
    assert_eq!(control.control(), "cancel");
    let admin = verify_admin_request(
        positive.admin.raw_admin_utf8.as_bytes(),
        &hex_array(&positive.admin.public_key_hex),
    )
    .unwrap();
    assert_eq!(admin.operation(), "enroll_issuer");
}

#[test]
fn every_negative_vector_refuses_before_trusted_construction() {
    let positive: Positive =
        serde_json::from_slice(&fs::read(vectors().join("positive.json")).unwrap()).unwrap();
    let key = hex_array(&positive.activation.public_key_hex);
    let negative: Negative =
        serde_json::from_slice(&fs::read(vectors().join("negative.json")).unwrap()).unwrap();
    assert_eq!(negative.cases.len(), 26);
    assert_eq!(
        negative.zero_state,
        ["session", "journal", "hook", "tool", "effect"]
    );
    for case in negative.cases {
        let raw = match (case.raw_utf8, case.raw_base64) {
            (Some(text), None) => text.into_bytes(),
            (None, Some(encoded)) => STANDARD.decode(encoded).unwrap(),
            _ => panic!("{} must contain exactly one raw encoding", case.name),
        };
        let error = verify_activation_frame(&raw, &key).unwrap_err();
        assert_eq!(error.reason(), case.reason, "negative vector {}", case.name);
    }
}

#[test]
fn cryptographic_and_resource_boundaries_fail_closed() {
    let positive: Positive =
        serde_json::from_slice(&fs::read(vectors().join("positive.json")).unwrap()).unwrap();
    let key = hex_array(&positive.activation.public_key_hex);
    let raw = &positive.activation.raw_frame_utf8;

    let invalid_signature = raw.replace(
        "RATpLIeYiK-GIx9anHUhEki7HScOkRnR5DtNPP2iQLEsxQ3IuluvymXV_wr2MEaqg80RdoWYvM_raoCgR6NNDg",
        &"A".repeat(86),
    );
    assert_eq!(
        verify_activation_frame(invalid_signature.as_bytes(), &key)
            .unwrap_err()
            .reason(),
        RejectReason::InvalidSignature
    );

    let wrong_key = hex_array(&positive.rfc8032.public_key_hex);
    assert_eq!(
        verify_activation_frame(raw.as_bytes(), &wrong_key)
            .unwrap_err()
            .reason(),
        RejectReason::InvalidSignature
    );

    let noncanonical = raw.replacen(
        "\"waylandNanoActivation\":{",
        "\"waylandNanoActivation\":{ ",
        1,
    );
    assert_eq!(
        verify_activation_frame(noncanonical.as_bytes(), &key)
            .unwrap_err()
            .reason(),
        RejectReason::NoncanonicalPayload
    );

    let mut oversized = raw.clone();
    oversized.push_str(&" ".repeat(32 * 1024));
    assert_eq!(
        verify_activation_frame(oversized.as_bytes(), &key)
            .unwrap_err()
            .reason(),
        RejectReason::CarrierOversized
    );

    let deep = format!("{}0{}", "[".repeat(10), "]".repeat(10));
    assert_eq!(
        verify_activation_frame(deep.as_bytes(), &key)
            .unwrap_err()
            .reason(),
        RejectReason::MalformedJson
    );
    let wide = format!(
        "{{{}}}",
        (0..257)
            .map(|index| format!("\"k{index}\":0"))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert_eq!(
        verify_activation_frame(wide.as_bytes(), &key)
            .unwrap_err()
            .reason(),
        RejectReason::MalformedJson
    );

    // Signing the activation payload under the receipt domain proves domain separation.
    let frame: serde_json::Value = serde_json::from_str(raw).unwrap();
    let mut payload = frame["params"]["_meta"]["waylandNanoActivation"].clone();
    payload.as_object_mut().unwrap().remove("signature");
    let canonical = serde_jcs::to_vec(&payload).unwrap();
    let mut wrong_domain_message = b"WAYLAND-NANO-RECEIPT\0v1\0".to_vec();
    wrong_domain_message.extend_from_slice(&canonical);
    let wrong_domain_signature =
        SigningKey::from_bytes(&std::array::from_fn(|i| (i + 1) as u8)).sign(&wrong_domain_message);
    let encoded =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(wrong_domain_signature.to_bytes());
    let wrong_domain = raw.replace(
        "RATpLIeYiK-GIx9anHUhEki7HScOkRnR5DtNPP2iQLEsxQ3IuluvymXV_wr2MEaqg80RdoWYvM_raoCgR6NNDg",
        &encoded,
    );
    assert_eq!(
        verify_activation_frame(wrong_domain.as_bytes(), &key)
            .unwrap_err()
            .reason(),
        RejectReason::InvalidSignature
    );
}
