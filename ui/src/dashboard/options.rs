use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    List,
    Grid,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    Start,
    #[default]
    Center,
    End,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub mode: Mode,
    pub horizontal_alignment: Alignment,
    pub vertical_alignment: Alignment,
    pub width: u32,
    pub height: u32,
    pub rows: u32,
    pub columns: u32,
    pub row_height: u32,
    pub card_width: u32,
    pub card_height: u32,
    pub device_card_width: u32,
    pub device_card_height: u32,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::List,
            horizontal_alignment: Alignment::Center,
            vertical_alignment: Alignment::Center,
            width: 440,
            height: 400,
            rows: 3,
            columns: 2,
            row_height: 40,
            card_width: 180,
            card_height: 140,
            device_card_width: 272,
            device_card_height: 340,
        }
    }
}
impl Options {
    pub fn load() -> Self {
        std::fs::read(crate::backend::data_dir().join("preferences.json"))
            .ok()
            .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
            .and_then(|v| serde_json::from_value::<Self>(v["dashboard"].clone()).ok())
            .filter(|v| v.validate().is_ok())
            .unwrap_or_default()
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (320..=960).contains(&self.width) && (240..=800).contains(&self.height),
            "弹窗大小须为 320–960 × 240–800"
        );
        ensure!(
            (1..=8).contains(&self.rows) && (1..=4).contains(&self.columns),
            "行数须为 1–8，列数须为 1–4"
        );
        ensure!((32..=64).contains(&self.row_height), "项目行高须为 32–64");
        ensure!(
            (100..=400).contains(&self.card_width) && (80..=360).contains(&self.card_height),
            "速览卡片尺寸须为 100–400 × 80–360"
        );
        ensure!(
            (240..=400).contains(&self.device_card_width)
                && (300..=480).contains(&self.device_card_height),
            "设备卡片尺寸须为 240–400 × 300–480"
        );
        Ok(())
    }
}
/// Preserve unrelated settings when either the taskbar or dashboard changes.
pub fn save_preference(key: &str, value: Value) -> Result<()> {
    let dir = crate::backend::data_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("preferences.json");
    let mut prefs = if path.exists() {
        serde_json::from_slice::<Value>(&std::fs::read(&path)?)?
    } else {
        json!({})
    };
    ensure!(prefs.is_object(), "invalid UI preferences");
    prefs[key] = value;
    let temporary = dir.join(format!("preferences-{}.tmp", uuid::Uuid::new_v4()));
    use std::io::Write;
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec(&prefs)?)?;
    file.sync_all()?;
    drop(file);
    let result = std::fs::rename(&temporary, &path);
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_accepts_dense_layout_for_proportional_fitting_and_checks_card_sizes() {
        let mut options = Options {
            width: 320,
            columns: 4,
            mode: Mode::Grid,
            ..Default::default()
        };
        assert!(options.validate().is_ok());
        options.mode = Mode::List;
        assert!(options.validate().is_ok());
        options.height = 240;
        options.rows = 8;
        assert!(options.validate().is_ok());
        options.mode = Mode::Grid;
        assert!(options.validate().is_ok());
        options.mode = Mode::List;
        options.row_height = 31;
        assert!(options.validate().is_err());
    }
}
