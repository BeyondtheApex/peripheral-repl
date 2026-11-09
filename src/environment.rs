//! Host presentation preferences and bounded plugin translation resources.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Catalog = BTreeMap<String, BTreeMap<String, String>>;
#[derive(Default)]
pub struct Translations {
    pub catalog: Catalog,
    pub default_locale: String,
}
impl Translations {
    pub fn text(&self, locale: &str, key: &str) -> String {
        translate(&self.catalog, locale, &self.default_locale, key)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Environment {
    pub locale: String,
    pub theme: String,
    pub accent: String,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            locale: "zh-CN".into(),
            theme: "dark".into(),
            accent: "#8ca8ff".into(),
        }
    }
}
pub fn valid_locale(locale: &str) -> bool {
    !locale.is_empty()
        && locale.len() <= 32
        && locale
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
}
impl Environment {
    pub fn validate(&self) -> Result<()> {
        ensure!(valid_locale(&self.locale), "invalid locale");
        ensure!(
            matches!(self.theme.as_str(), "light" | "dark"),
            "theme must be light or dark"
        );
        ensure!(
            self.accent.len() == 7 && self.accent.starts_with('#'),
            "accent must be #RRGGBB"
        );
        peripheral_ui_protocol::color(&self.accent)?;
        Ok(())
    }
    pub fn colors(&self) -> BTreeMap<&'static str, String> {
        let dark = self.theme == "dark";
        [
            ("background", if dark { "#11151b" } else { "#f3f5fa" }),
            ("surface", if dark { "#1a202a" } else { "#ffffff" }),
            ("elevated", if dark { "#243044" } else { "#e5ebf5" }),
            ("text", if dark { "#e7edf7" } else { "#172033" }),
            ("muted", if dark { "#96a2b7" } else { "#536279" }),
            ("border", if dark { "#354156" } else { "#ccd5e3" }),
            ("success", if dark { "#36cc8b" } else { "#087c50" }),
            ("warning", if dark { "#e8bc71" } else { "#8a5900" }),
            ("error", if dark { "#f08075" } else { "#b42b24" }),
            ("accent", self.accent.as_str()),
        ]
        .into_iter()
        .map(|(k, v)| (k, v.into()))
        .collect()
    }
    pub fn value(&self) -> serde_json::Value {
        serde_json::json!({"locale":self.locale,"theme":self.theme,"accent":self.accent,"colors":self.colors()})
    }
}
pub fn validate_catalog(catalog: &Catalog, default_locale: &str) -> Result<()> {
    ensure!(valid_locale(default_locale), "invalid default_locale");
    ensure!(catalog.len() <= 16, "at most 16 locales");
    for (locale, entries) in catalog {
        ensure!(
            valid_locale(locale) && entries.len() <= 512,
            "invalid locale resource"
        );
        for (key, value) in entries {
            ensure!(
                !key.is_empty() && key.len() <= 128 && value.len() <= 2048,
                "translation key/value exceeds limit"
            );
        }
    }
    ensure!(
        serde_json::to_vec(catalog)?.len() <= 24576,
        "locale resources exceed 24 KiB"
    );
    Ok(())
}
pub fn translate(catalog: &Catalog, locale: &str, default_locale: &str, key: &str) -> String {
    let base = locale.split('-').next().unwrap_or(locale);
    [locale, base, default_locale]
        .into_iter()
        .find_map(|l| catalog.get(l).and_then(|c| c.get(key)))
        .cloned()
        .unwrap_or_else(|| key.into())
}

pub fn install(lua: &mlua::Lua, ctx: &crate::bindings::Context) -> mlua::Result<()> {
    use mlua::{Function, LuaSerdeExt, Table};
    let (catalog, fallback) = if ctx.name.starts_with("plugin_") {
        let manifest = crate::plugins::package::read(&ctx.root).map_err(mlua::Error::external)?;
        (manifest.locales, manifest.default_locale)
    } else {
        (Catalog::new(), "zh-CN".into())
    };
    *ctx.translations.borrow_mut() = Translations {
        catalog,
        default_locale: fallback,
    };
    let i18n = lua.create_table()?;
    let c = ctx.clone();
    i18n.set(
        "locale",
        lua.create_function(move |_, ()| Ok(c.shared.environment.lock().unwrap().locale.clone()))?,
    )?;
    let c = ctx.clone();
    i18n.set(
        "t",
        lua.create_function(move |_, (key, params): (String, Option<Table>)| {
            if key.len() > 128 {
                return Err(mlua::Error::external("translation key too long"));
            }
            let locale = c.shared.environment.lock().unwrap().locale.clone();
            let text = c.translations.borrow().text(&locale, &key);
            let mut arguments = BTreeMap::new();
            if let Some(params) = params {
                for pair in params.pairs::<String, mlua::Value>() {
                    let (k, v) = pair?;
                    if arguments.len() >= 32 || k.len() > 64 {
                        return Err(mlua::Error::external("too many interpolation arguments"));
                    }
                    let v = match v {
                        mlua::Value::String(s) => s.to_str()?.to_owned(),
                        mlua::Value::Integer(i) => i.to_string(),
                        mlua::Value::Number(n) => n.to_string(),
                        _ => {
                            return Err(mlua::Error::external(
                                "translation arguments must be strings or numbers",
                            ));
                        }
                    };
                    if v.len() > 1024 {
                        return Err(mlua::Error::external("translation argument too long"));
                    }
                    arguments.insert(k, v);
                }
            }
            // Single pass substitution: inserted text is never interpreted as another template.
            let mut out = String::new();
            let mut rest = text.as_str();
            while let Some(start) = rest.find('{') {
                out.push_str(&rest[..start]);
                rest = &rest[start..];
                let Some(end) = rest.find('}') else { break };
                out.push_str(
                    arguments
                        .get(&rest[1..end])
                        .map(String::as_str)
                        .unwrap_or(&rest[..=end]),
                );
                rest = &rest[end + 1..];
                if out.len() > 4096 {
                    return Err(mlua::Error::external("translated text too long"));
                }
            }
            out.push_str(rest);
            if out.len() > 4096 {
                return Err(mlua::Error::external("translated text too long"));
            }
            Ok(out)
        })?,
    )?;
    lua.globals().set("i18n", i18n)?;
    let ui: Table = lua.globals().get("ui")?;
    let c = ctx.clone();
    ui.set(
        "environment",
        lua.create_function(move |lua, ()| {
            lua.to_value(&c.shared.environment.lock().unwrap().value())
        })?,
    )?;
    let c = ctx.clone();
    ui.set(
        "on_environment",
        lua.create_function(move |_, f: Function| {
            *c.environment_callback.borrow_mut() = Some(f);
            Ok(())
        })?,
    )?;
    Ok(())
}
