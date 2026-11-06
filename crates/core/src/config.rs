use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::rules::Safety;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeleteMode {
    #[default]
    Trash,
    Permanent,
}

impl DeleteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DeleteMode::Trash => "trash",
            DeleteMode::Permanent => "permanent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "trash" => Some(DeleteMode::Trash),
            "permanent" => Some(DeleteMode::Permanent),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct McpConfig {
    /// Safety classes an agent may execute without TUI approval.
    pub auto_approve: Vec<Safety>,
    /// Lets agents propose `review` items younger than the stale threshold.
    pub allow_review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CleanupConfig {
    pub mode: DeleteMode,
    pub max_bytes_per_execution: u64,
    pub max_items: usize,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            mode: DeleteMode::Trash,
            max_bytes_per_execution: 100_000_000_000,
            max_items: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub default_root: Option<PathBuf>,
    pub one_file_system: bool,
    pub theme: String,
    pub icons: bool,
    pub stale_days: i64,
    pub keep_scans: usize,
    pub rules_path: Option<PathBuf>,
    pub mcp: McpConfig,
    pub cleanup: CleanupConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_root: None,
            one_file_system: true,
            theme: "dark".into(),
            icons: false,
            stale_days: 90,
            keep_scans: 5,
            rules_path: None,
            mcp: McpConfig::default(),
            cleanup: CleanupConfig::default(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        crate::paths::config_dir().join("config.toml")
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                toml::from_str(&s).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn rules_path(&self) -> PathBuf {
        self.rules_path
            .clone()
            .unwrap_or_else(|| crate::paths::config_dir().join("rules.toml"))
    }

    pub fn default_root(&self) -> PathBuf {
        self.default_root
            .clone()
            .unwrap_or_else(crate::paths::home_dir)
    }
}
