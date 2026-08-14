use crate::{KeyMods, MouseButton, Region, ScreenshotFormat};
use serde::{Deserialize, Serialize};

pub const NANO_CUA_OP_LOCKED_VARIANT_COUNT: usize = 11;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CuaOp {
    LeftClick {
        x: i32,
        y: i32,
        #[serde(default)]
        button: MouseButton,
        #[serde(default)]
        mods: KeyMods,
    },
    RightClick {
        x: i32,
        y: i32,
        #[serde(default)]
        mods: KeyMods,
    },
    DoubleClick {
        x: i32,
        y: i32,
        #[serde(default)]
        button: MouseButton,
    },
    MouseMove {
        x: i32,
        y: i32,
    },
    Scroll {
        x: i32,
        y: i32,
        dx: i32,
        dy: i32,
    },
    Type {
        text: String,
    },
    Key {
        keys: String,
        #[serde(default)]
        mods: KeyMods,
    },
    Screenshot {
        #[serde(default)]
        region: Region,
        #[serde(default)]
        format: ScreenshotFormat,
        #[serde(default)]
        redact: bool,
    },
    AxTree {},
    Wait {
        duration_ms: u64,
    },
    FrontmostApp {},
}

impl CuaOp {
    pub fn kind_tag(&self) -> &'static str {
        match self {
            Self::LeftClick { .. } => "left_click",
            Self::RightClick { .. } => "right_click",
            Self::DoubleClick { .. } => "double_click",
            Self::MouseMove { .. } => "mouse_move",
            Self::Scroll { .. } => "scroll",
            Self::Type { .. } => "type",
            Self::Key { .. } => "key",
            Self::Screenshot { .. } => "screenshot",
            Self::AxTree {} => "ax_tree",
            Self::Wait { .. } => "wait",
            Self::FrontmostApp {} => "frontmost_app",
        }
    }

    /// Only these operations may be exposed to the model in v1.
    pub fn is_v1_model_surface(&self) -> bool {
        !matches!(
            self,
            Self::MouseMove { .. } | Self::AxTree {} | Self::FrontmostApp {}
        )
    }

    #[doc(hidden)]
    pub fn all_variants_for_test() -> Vec<Self> {
        vec![
            Self::LeftClick {
                x: 1,
                y: 2,
                button: MouseButton::Left,
                mods: KeyMods::default(),
            },
            Self::RightClick {
                x: 1,
                y: 2,
                mods: KeyMods::default(),
            },
            Self::DoubleClick {
                x: 1,
                y: 2,
                button: MouseButton::Left,
            },
            Self::MouseMove { x: 1, y: 2 },
            Self::Scroll {
                x: 1,
                y: 2,
                dx: 0,
                dy: 1,
            },
            Self::Type {
                text: "hello".into(),
            },
            Self::Key {
                keys: "ctrl+t".into(),
                mods: KeyMods::default(),
            },
            Self::Screenshot {
                region: Region::Full,
                format: ScreenshotFormat::Png,
                redact: true,
            },
            Self::AxTree {},
            Self::Wait { duration_ms: 1 },
            Self::FrontmostApp {},
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CuaOpResult {
    Ok,
    Screenshot {
        format: ScreenshotFormat,
        data_b64: String,
        width: u32,
        height: u32,
        redacted: bool,
    },
    FrontmostApp {
        app_id: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locked_enum_roundtrips_and_has_no_forbidden_variants() {
        let ops = CuaOp::all_variants_for_test();
        assert_eq!(ops.len(), NANO_CUA_OP_LOCKED_VARIANT_COUNT);
        for op in ops {
            let json = serde_json::to_string(&op).unwrap();
            assert_eq!(serde_json::from_str::<CuaOp>(&json).unwrap(), op);
        }
        let tags = CuaOp::all_variants_for_test()
            .iter()
            .map(CuaOp::kind_tag)
            .collect::<Vec<_>>();
        assert!(!tags.iter().any(|tag| tag.contains("drag")));
    }
    #[test]
    fn model_surface_has_exactly_eight_ops() {
        assert_eq!(
            CuaOp::all_variants_for_test()
                .iter()
                .filter(|op| op.is_v1_model_surface())
                .count(),
            8
        );
    }
}
