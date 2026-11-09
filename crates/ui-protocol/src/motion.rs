use crate::Kind;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScaleMode {
    /// Fit intrinsic content within the viewport without enlargement.
    #[default]
    Shrink,
    /// Fit and allow enlargement, preserving aspect ratio.
    Contain,
    None,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnimationKind {
    Spin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Animation {
    pub kind: AnimationKind,
    #[serde(default = "duration")]
    pub duration_ms: u64,
    #[serde(default = "fps")]
    pub fps: u32,
}
fn duration() -> u64 {
    1600
}
fn fps() -> u32 {
    30
}
impl Animation {
    pub fn validate(&self, kind: Kind) -> Result<()> {
        ensure!(kind == Kind::Ring, "spin animation requires a ring node");
        ensure!(
            (250..=60000).contains(&self.duration_ms),
            "animation duration must be 250..60000 ms"
        );
        ensure!((1..=30).contains(&self.fps), "animation fps must be 1..30");
        Ok(())
    }
}
