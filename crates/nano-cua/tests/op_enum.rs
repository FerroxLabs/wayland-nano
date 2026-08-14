//! Locked-variant-count guard + serde roundtrip for every `CuaOp`
//! variant (donor `tests/op_enum_test.rs` port). Bumping the variant
//! count requires re-auditing the S9 design (§1.2 / §9).

use nano_cua::{CuaOp, NANO_CUA_OP_LOCKED_VARIANT_COUNT};

#[test]
fn op_count_matches_locked_constant() {
    let variants = CuaOp::all_variants_for_test();
    assert_eq!(
        variants.len(),
        NANO_CUA_OP_LOCKED_VARIANT_COUNT,
        "CuaOp variant count drifted from the locked constant — S9 design audit required"
    );
}

#[test]
fn op_serde_roundtrip_every_variant() {
    for op in CuaOp::all_variants_for_test() {
        let s = serde_json::to_string(&op).expect("op serializes");
        let back: CuaOp = serde_json::from_str(&s).expect("op deserializes");
        assert_eq!(op, back, "roundtrip failed for {s}");
    }
}

#[test]
fn op_kind_tags_are_unique_and_snake_case() {
    let mut tags = Vec::new();
    for op in CuaOp::all_variants_for_test() {
        let tag = op.kind_tag();
        assert!(
            tag.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "kind tag {tag:?} is not snake_case"
        );
        tags.push(tag);
    }
    let unique: std::collections::HashSet<_> = tags.iter().copied().collect();
    assert_eq!(unique.len(), tags.len(), "duplicate kind tags: {tags:?}");
}

#[test]
fn op_serializes_with_kind_tag() {
    let op = CuaOp::Wait { duration_ms: 100 };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["kind"], "wait");
    assert_eq!(v["duration_ms"], 100);
}

#[test]
fn no_drag_variant_and_exactly_eight_model_ops() {
    let ops = CuaOp::all_variants_for_test();
    for op in &ops {
        assert!(
            !op.kind_tag().contains("drag"),
            "v1 forbids drag operations (donor's own security omission)"
        );
    }
    let surface: Vec<&str> = ops
        .iter()
        .filter(|op| op.is_v1_model_surface())
        .map(CuaOp::kind_tag)
        .collect();
    assert_eq!(
        surface,
        [
            "left_click",
            "right_click",
            "double_click",
            "scroll",
            "type",
            "key",
            "screenshot",
            "wait"
        ],
        "the v1 model surface is the ruled 8-op subset, in wire order"
    );
}
