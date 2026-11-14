//! User-selected privilege mode for fresh backend starts.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Preferences {
    pub start_elevated: bool,
}

impl Preferences {
    pub fn load() -> Result<Self> {
        Self::load_from(&super::data_dir().join("preferences.json"))
    }

    fn load_from(path: &Path) -> Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).context("Cannot read backend startup preferences"),
        };
        let value: Value = serde_json::from_slice(&bytes).context("Invalid UI preferences")?;
        ensure!(value.is_object(), "Invalid UI preferences");
        match value.get("backend") {
            None => Ok(Self::default()),
            Some(value) => {
                serde_json::from_value(value.clone()).context("Invalid backend startup preferences")
            }
        }
    }

    pub fn save(self) -> Result<()> {
        crate::dashboard::save_preference("backend", serde_json::to_value(self)?)
    }

    pub fn start_action(self) -> &'static str {
        if self.start_elevated {
            "elevate"
        } else {
            "start"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_mode_defaults_to_normal_and_rejects_invalid_settings() {
        let path = std::env::temp_dir().join(format!(
            "peripheral-preferences-{}.json",
            uuid::Uuid::new_v4()
        ));
        assert_eq!(
            Preferences::load_from(&path).unwrap().start_action(),
            "start"
        );
        for (content, expected) in [
            (r#"{"dashboard":{"columns":2}}"#, Some("start")),
            (r#"{"backend":{"start_elevated":true}}"#, Some("elevate")),
            (r#"{"backend":{"start_elevated":false}}"#, Some("start")),
            (r#"{"backend":{"start_elevated":"false"}}"#, None),
            (r#"{"backend":null}"#, None),
            ("[]", None),
            ("{", None),
        ] {
            std::fs::write(&path, content).unwrap();
            assert_eq!(
                Preferences::load_from(&path)
                    .ok()
                    .map(Preferences::start_action),
                expected
            );
        }
        std::fs::remove_file(path).unwrap();
    }
}
