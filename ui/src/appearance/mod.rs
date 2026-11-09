//! Application appearance policy shared by all host-owned surfaces.
mod commands;
pub mod settings;
use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};
use peripheral_repl::environment::Environment;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::BTreeMap};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub mode: String,
    pub accent: String,
    pub language: String,
    pub material: String,
    pub opacity: u32,
    pub hide_titlebar: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            mode: "system".into(),
            accent: "#8ca8ff".into(),
            language: "system".into(),
            material: "blur".into(),
            opacity: 82,
            hide_titlebar: false,
        }
    }
}
impl Options {
    pub fn load() -> Self {
        std::fs::read(crate::backend::data_dir().join("preferences.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| serde_json::from_value::<Self>(v["appearance"].clone()).ok())
            .filter(|v| v.validate().is_ok())
            .unwrap_or_default()
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            ["system", "light", "dark"].contains(&self.mode.as_str()),
            "invalid theme mode"
        );
        anyhow::ensure!(
            ["system", "zh-CN", "en"].contains(&self.language.as_str()),
            "invalid language"
        );
        anyhow::ensure!(
            ["solid", "transparent", "blur", "mica"].contains(&self.material.as_str()),
            "invalid window material"
        );
        anyhow::ensure!(
            (50..=100).contains(&self.opacity),
            "opacity must be 50..100"
        );
        self.environment().validate()
    }
    pub fn environment(&self) -> Environment {
        let system = crate::native::appearance::system();
        Environment {
            locale: if self.language == "system" {
                system.locale
            } else {
                self.language.clone()
            },
            theme: if self.mode == "system" {
                if system.dark { "dark" } else { "light" }.into()
            } else {
                self.mode.clone()
            },
            accent: self.accent.clone(),
        }
    }
}
#[derive(Clone)]
struct Current {
    options: Options,
    env: Environment,
    palette: BTreeMap<&'static str, Hsla>,
    system_transparency: bool,
    material_available: bool,
}
fn palette(env: &Environment) -> BTreeMap<&'static str, Hsla> {
    env.colors()
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                rgba(peripheral_ui_protocol::color(&value).expect("validated theme color")).into(),
            )
        })
        .collect()
}
thread_local! {static CURRENT:RefCell<Current>=RefCell::new(Current{options:Options::default(),env:Environment::default(),palette:palette(&Environment::default()),system_transparency:false,material_available:false});}
pub fn options() -> Options {
    CURRENT.with(|v| v.borrow().options.clone())
}
pub fn environment() -> Environment {
    CURRENT.with(|v| v.borrow().env.clone())
}
pub fn apply(options: &Options, cx: &mut App) {
    let env = options.environment();
    let system = crate::native::appearance::system();
    CURRENT.with(|v| {
        *v.borrow_mut() = Current {
            options: options.clone(),
            palette: palette(&env),
            env: env.clone(),
            system_transparency: system.transparency,
            material_available: system.transparency,
        }
    });
    Theme::change(
        if env.theme == "dark" {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    let theme = Theme::global_mut(cx);
    let accent = rgba(peripheral_ui_protocol::color(&env.accent).unwrap_or(0x8ca8ffff)).into();
    theme.colors.primary = accent;
    theme.colors.primary_hover = accent;
    theme.colors.primary_active = accent;
    theme.colors.button_primary = accent;
    theme.colors.button_primary_hover = accent;
    theme.colors.button_primary_active = accent;
    theme.colors.ring = accent;
    theme.colors.sidebar_primary = accent;
    crate::i18n::set_locale(&env.locale);
    // Built-in component translations share the host locale.
    gpui_kit::component::set_locale(if env.locale.starts_with("zh") {
        "zh-CN"
    } else {
        "en"
    });
}
pub fn unavailable_material() {
    CURRENT.with(|v| v.borrow_mut().material_available = false);
}
pub fn system_changed() -> bool {
    let system = crate::native::appearance::system();
    CURRENT.with(|v| {
        let v = v.borrow();
        v.options.environment() != v.env || v.system_transparency != system.transparency
    })
}
pub fn material_enabled() -> bool {
    CURRENT.with(|v| {
        let v = v.borrow();
        v.material_available && v.options.material != "solid"
    })
}
pub fn window_background(popup: bool) -> WindowBackgroundAppearance {
    if !material_enabled() {
        return WindowBackgroundAppearance::Transparent;
    }
    CURRENT.with(|v| match v.borrow().options.material.as_str() {
        "blur" => WindowBackgroundAppearance::Blurred,
        "mica" if popup => WindowBackgroundAppearance::Blurred,
        "mica" => WindowBackgroundAppearance::MicaBackdrop,
        "transparent" => WindowBackgroundAppearance::Transparent,
        _ => WindowBackgroundAppearance::Transparent,
    })
}
/// Interior panels tint an already composited backdrop; an opaque card would
/// hide the material across most of the management and dashboard windows.
pub fn panel(name: &str) -> Hsla {
    let mut color = token(name);
    if material_enabled() {
        color.a = CURRENT.with(|v| v.borrow().options.opacity as f32 / 100.) * 0.5;
    }
    color
}
pub fn token(name: &str) -> Hsla {
    CURRENT.with(|v| {
        v.borrow()
            .palette
            .get(name)
            .copied()
            .unwrap_or_else(|| rgb(0x808080).into())
    })
}
pub fn background() -> Hsla {
    let mut c = token("background");
    if material_enabled() {
        c.a = options().opacity as f32 / 100.;
    }
    c
}
pub fn popup_background(custom: Option<&String>) -> Hsla {
    let mut c = custom
        .and_then(|c| peripheral_ui_protocol::color(c).ok())
        .map(|c| rgba(c).into())
        .unwrap_or_else(|| token("surface"));
    if material_enabled() {
        c.a *= options().opacity as f32 / 100.;
    } else {
        c.a = 1.;
    }
    c
}
/// Migration bridge for host palette constants; plugin colors never pass through this map.
pub fn color(value: u32) -> Hsla {
    if value == 0x3d2b2c || value == 0x42352b {
        let mut c = token(if value == 0x3d2b2c {
            "error"
        } else {
            "warning"
        });
        c.a = 0.15;
        return c;
    }
    let name = match value {
        0x11151b | 0x141d29 => "background",
        0x151a22 | 0x1a202a => "surface",
        0x222d3d | 0x243044 | 0x253247 | 0x263042 | 0x28334a | 0x293349 | 0x29374e | 0x2c394e
        | 0x3a4862 => "elevated",
        0xe7edf7 | 0xeff3fa => "text",
        0x96a2b7 | 0x9cabc0 | 0x8793a7 => "muted",
        0x354156 | 0x2a3342 | 0x465063 => "border",
        0x9aafff | 0x8ca8ff => "accent",
        0x36cc8b | 0x3ed99a | 0x348d71 => "success",
        0xe8bc71 | 0xe7b85d => "warning",
        0xf08075 | 0xf19b7c => "error",
        _ => return rgb(value).into(),
    };
    if name == "surface" || name == "elevated" {
        panel(name)
    } else {
        token(name)
    }
}
