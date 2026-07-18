use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub ui: Ui,
    pub open: Open,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct General {
    /// Base branch. "auto" picks develop if it exists, otherwise the default branch.
    pub base: String,
    /// Polling fallback interval (ms)
    pub poll_ms: u64,
    /// Width threshold (columns) below which the UI drops to a single column
    pub narrow_cols: u16,
}

impl Default for General {
    fn default() -> Self {
        Self {
            base: "auto".into(),
            poll_ms: 2000,
            narrow_cols: 100,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Ui {
    /// Nerd Font icons
    pub icons: bool,
    /// Tree column width (%)
    pub split_pct: u16,
    /// syntect theme name
    pub theme: String,
    /// Wrap long diff lines (toggled with the w key)
    pub wrap: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            icons: true,
            split_pct: 38,
            theme: "base16-ocean.dark".into(),
            wrap: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Open {
    /// Opener name used by the e key and follow mode
    pub default: String,
    /// Named openers (command templates)
    #[serde(flatten)]
    pub openers: BTreeMap<String, Opener>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Opener {
    pub cmd: Vec<String>,
}

pub fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/wtd/config.toml"))
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str::<Config>(&text) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("wtd: failed to load config.toml ({e}); using defaults");
                Self::default()
            }
        }
    }

    pub fn default_opener(&self) -> Option<&Opener> {
        self.open.openers.get(&self.open.default)
    }
}
