//! App settings, kept outside the game's save file.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// Highest anti-aliasing regardless of the game's quality option.
    #[default]
    Best,
    /// Honour the in-game LOW / MEDIUM / HIGH option, as in 2001.
    Game,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Hotspots {
    /// Highlight clickable areas while Space is held.
    #[default]
    HoldSpace,
    Always,
    Off,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Settings {
    pub game_path: Option<PathBuf>,
    /// 0–100.
    pub music_volume: u8,
    /// `None` follows the game's own display option; F11 sets it.
    pub fullscreen: Option<bool>,
    pub quality: Quality,
    pub hotspots: Hotspots,
    /// A .sf2 to use instead of the bundled GeneralUser GS.
    pub soundfont: Option<PathBuf>,
    /// 1–9 are save slots, 0 is the autosave.
    pub slot: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            game_path: None,
            music_volume: 80,
            fullscreen: None,
            quality: Quality::Best,
            hotspots: Hotspots::HoldSpace,
            soundfont: None,
            slot: 1,
        }
    }
}

impl Settings {
    fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("zlaty-kalich").join("settings.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::error!("Ignoring {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        let result = std::fs::create_dir_all(path.parent().unwrap())
            .map_err(anyhow::Error::from)
            .and_then(|()| Ok(std::fs::write(&path, toml::to_string_pretty(self)?)?));
        if let Err(e) = result {
            tracing::error!("Couldn't save {}: {e}", path.display());
        }
    }
}
