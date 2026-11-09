use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct ResourceRef {
    pub path: String,
    pub sha256: String,
}
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    #[default]
    Contain,
    Cover,
    Fill,
}
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Clip {
    #[default]
    None,
    Rounded,
    Circle,
    Superellipse,
    Bevel,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ImageSpec {
    pub resource: String,
    pub fit: Fit,
    pub clip: Clip,
    /// Corner radius in logical pixels; circle uses half the shorter edge.
    pub radius: f32,
}
impl Default for ImageSpec {
    fn default() -> Self {
        Self {
            resource: String::new(),
            fit: Fit::Contain,
            clip: Clip::None,
            radius: 12.,
        }
    }
}
impl ImageSpec {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            super::identifier(&self.resource),
            "invalid image resource ID"
        );
        ensure!(
            self.radius.is_finite() && (0. ..=1000.).contains(&self.radius),
            "invalid image radius"
        );
        Ok(())
    }
}
