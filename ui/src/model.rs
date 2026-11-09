use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    Window,
    Overlay,
    Popup,
    TaskbarPreview,
    Texture,
    Xaml,
    Taskbar,
    Tray,
}
