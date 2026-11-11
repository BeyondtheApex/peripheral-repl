//! Versioned, renderer-independent Lua UI descriptions. No GPUI or device I/O.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const VERSION: u32 = 6;
mod motion;
pub use motion::{Animation, AnimationKind, ScaleMode};
mod images;
pub use images::{Clip, Fit, ImageSpec, ResourceRef};
pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Taskbar,
    Tray,
    Overlay,
    Window,
    Popup,
    TaskbarPreview,
    Texture,
    Xaml,
    Panel,
    TrayList,
    TrayCard,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    #[serde(default = "version")]
    pub version: u32,
    pub target: Target,
    #[serde(default)]
    pub title: String,
    #[serde(default = "width")]
    pub width: f32,
    #[serde(default = "height")]
    pub height: f32,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub events: Events,
    /// Popup ownership within the same session; removing/hiding a parent closes descendants.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default = "visible")]
    pub visible: bool,
    #[serde(default)]
    pub x: Option<f32>,
    #[serde(default)]
    pub y: Option<f32>,
    /// Fill the host bounds, independently of the fitted content tree.
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub scale: ScaleMode,
    /// Taskbar ascending, dashboard descending; ties use stable publication IDs.
    #[serde(default)]
    pub position_preference: [i32; 2],
    pub root: Node,
}
fn version() -> u32 {
    VERSION
}
fn visible() -> bool {
    true
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Events {
    pub left_click: Option<String>,
    pub right_click: Option<String>,
}
impl Events {
    pub fn action(&self, button: &str) -> Option<&str> {
        match button {
            "left" => self.left_click.as_deref(),
            "right" => self.right_click.as_deref(),
            _ => None,
        }
    }
    fn validate(&self) -> Result<()> {
        for action in [&self.left_click, &self.right_click].into_iter().flatten() {
            ensure!(identifier(action), "invalid UI event action");
        }
        Ok(())
    }
}
fn width() -> f32 {
    48.
}
fn height() -> f32 {
    40.
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub image: Option<ImageSpec>,
    #[serde(default)]
    pub value: f32,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub events: Events,
    #[serde(default)]
    pub style: Style,
    #[serde(default)]
    pub animation: Option<Animation>,
    #[serde(default)]
    pub children: Vec<Node>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Row,
    Column,
    Text,
    Icon,
    Progress,
    Ring,
    Spacer,
    Button,
    Image,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Style {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub size: Option<f32>,
    pub padding: Option<f32>,
    pub gap: Option<f32>,
    pub radius: Option<f32>,
    pub stroke: Option<f32>,
    pub font_size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_align: Option<TextAlignment>,
    pub color: Option<String>,
    pub background: Option<String>,
    pub track: Option<String>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextAlignment {
    Left,
    Center,
    Right,
}
pub fn color(value: &str) -> Result<u32> {
    let s = value.strip_prefix('#').unwrap_or(value);
    ensure!(
        s.len() == 6 || s.len() == 8,
        "colors must be #RRGGBB or #RRGGBBAA"
    );
    let parsed = u32::from_str_radix(s, 16)?;
    Ok(if s.len() == 6 {
        (parsed << 8) | 255
    } else {
        parsed
    })
}
impl Surface {
    pub fn validate(&self) -> Result<()> {
        if let Some(background) = &self.background {
            color(background)?;
        }
        if let Some(radius) = self.radius {
            ensure!(
                radius.is_finite() && (0. ..=128.).contains(&radius),
                "invalid surface radius"
            );
        }
        self.events.validate()?;
        if let Some(parent) = &self.parent {
            ensure!(
                self.target == Target::Popup && identifier(parent),
                "only popups have a parent surface ID"
            );
        }
        // Legacy Lua documents remain valid; the host handshake uses VERSION
        // so an older renderer cannot silently ignore the ordering extension.
        ensure!(
            matches!(self.version, 5 | VERSION),
            "unsupported UI protocol version"
        );
        ensure!(self.title.len() <= 160, "UI title too long");
        ensure!(
            self.x.is_some() == self.y.is_some(),
            "position needs both x and y"
        );
        for v in [self.x, self.y].into_iter().flatten() {
            ensure!(
                v.is_finite() && v.abs() <= 32768.,
                "invalid surface position"
            );
        }
        ensure!(
            self.width.is_finite()
                && (16. ..=1000.).contains(&self.width)
                && self.height.is_finite()
                && (16. ..=800.).contains(&self.height),
            "invalid surface dimensions"
        );
        if self.target == Target::Taskbar {
            ensure!(
                self.width <= 320. && self.height <= 64.,
                "taskbar surface exceeds 320x64 logical pixels"
            );
        }
        if self.target == Target::Tray {
            ensure!(
                self.width <= 64. && self.height <= 64.,
                "tray surface exceeds 64x64"
            );
        }
        if let Some(action) = &self.action {
            ensure!(identifier(action), "invalid surface action");
        }
        let mut ids = BTreeSet::new();
        validate_node(&self.root, 0, &mut ids)?;
        ensure!(
            serde_json::to_vec(self)?.len() <= 32768,
            "UI description exceeds 32 KiB"
        );
        Ok(())
    }
    pub fn patch(&mut self, patch: &serde_json::Value) -> Result<()> {
        let mut candidate = serde_json::to_value(&*self)?;
        let nodes = patch
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("patch must map node IDs to changed fields"))?;
        for (id, changes) in nodes {
            ensure!(
                patch_node(&mut candidate["root"], id, changes)?,
                "unknown UI node: {id}"
            );
        }
        let parsed: Self = serde_json::from_value(candidate)?;
        parsed.validate()?;
        *self = parsed;
        Ok(())
    }
}
fn patch_node(node: &mut serde_json::Value, id: &str, changes: &serde_json::Value) -> Result<bool> {
    if node["id"] == id {
        let changes = changes
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("node patch must be an object"))?;
        for (key, value) in changes {
            ensure!(
                [
                    "text",
                    "icon",
                    "image",
                    "value",
                    "style",
                    "action",
                    "events",
                    "animation"
                ]
                .contains(&key.as_str()),
                "patch cannot change structure; republish to change layout"
            );
            if key == "style" {
                let fields = value
                    .as_object()
                    .ok_or_else(|| anyhow::anyhow!("style patch must be an object"))?;
                for (k, v) in fields {
                    node["style"][k] = v.clone();
                }
            } else {
                node[key] = value.clone();
            }
        }
        return Ok(true);
    }
    if let Some(children) = node["children"].as_array_mut() {
        for child in children {
            if patch_node(child, id, changes)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn validate_node(node: &Node, depth: usize, ids: &mut BTreeSet<String>) -> Result<()> {
    if let Some(animation) = &node.animation {
        animation.validate(node.kind)?;
    }
    if let Some(image) = &node.image {
        image.validate()?;
        ensure!(
            node.kind == Kind::Image,
            "image data requires an image node"
        );
    }
    ensure!(
        node.kind != Kind::Image || node.image.is_some(),
        "image node requires image data"
    );
    node.events.validate()?;
    ensure!(depth <= 8 && ids.len() < 128, "UI tree limit exceeded");
    ensure!(
        identifier(&node.id) && ids.insert(node.id.clone()),
        "invalid or duplicate node ID"
    );
    ensure!(node.text.len() <= 512, "UI text too long");
    ensure!(
        node.icon.is_empty() || (identifier(&node.icon) && node.icon.len() <= 48),
        "invalid icon name"
    );
    ensure!(
        node.value.is_finite() && (0. ..=100.).contains(&node.value),
        "UI progress must be 0..100"
    );
    for value in [
        node.style.width,
        node.style.height,
        node.style.size,
        node.style.padding,
        node.style.gap,
        node.style.radius,
        node.style.stroke,
        node.style.font_size,
    ]
    .into_iter()
    .flatten()
    {
        ensure!(
            value.is_finite() && (0. ..=1000.).contains(&value),
            "invalid style dimension"
        );
    }
    for value in [&node.style.color, &node.style.background, &node.style.track]
        .into_iter()
        .flatten()
    {
        color(value)?;
    }
    if let Some(action) = &node.action {
        ensure!(identifier(action), "invalid node action");
    }
    ensure!(
        matches!(
            node.kind,
            Kind::Row | Kind::Column | Kind::Ring | Kind::Button
        ) || node.children.is_empty(),
        "leaf node cannot contain children"
    );
    for child in &node.children {
        validate_node(child, depth + 1, ids)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn position_preferences_have_strict_shape_and_legacy_defaults() {
        let base =
            serde_json::json!({"version":5,"target":"taskbar","root":{"id":"r","type":"text"}});
        let legacy: Surface = serde_json::from_value(base.clone()).unwrap();
        legacy.validate().unwrap();
        assert_eq!(legacy.position_preference, [0, 0]);
        assert_eq!(
            serde_json::to_value(&legacy).unwrap()["position_preference"],
            serde_json::json!([0, 0])
        );
        for value in [
            serde_json::json!([1]),
            serde_json::json!([1, 2, 3]),
            serde_json::json!([0.5, 0]),
            serde_json::json!([2147483648_i64, 0]),
            serde_json::json!(["1", 0]),
            serde_json::Value::Null,
        ] {
            let mut description = base.clone();
            description["position_preference"] = value;
            assert!(serde_json::from_value::<Surface>(description).is_err());
        }
        let mut s = legacy;
        s.position_preference = [i32::MIN, i32::MAX];
        s.validate().unwrap();
        assert_eq!(
            serde_json::from_value::<Surface>(serde_json::to_value(&s).unwrap()).unwrap(),
            s
        );
    }
    #[test]
    fn bounded_atomic_patches() {
        let mut s:Surface=serde_json::from_value(serde_json::json!({"target":"taskbar","root":{"id":"battery","type":"ring","value":42}})).unwrap();
        s.validate().unwrap();
        let original = s.clone();
        assert!(
            s.patch(&serde_json::json!({"battery":{"value":200}}))
                .is_err()
        );
        assert_eq!(s, original);
        s.patch(&serde_json::json!({"battery":{"value":80,"style":{"color":"#22cc66"}}}))
            .unwrap();
        assert_eq!(s.root.value, 80.);
        assert!(
            s.patch(&serde_json::json!({"missing":{"text":"bad"}}))
                .is_err()
        );
    }
}
