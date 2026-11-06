use std::path::PathBuf;

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn from_env(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `~/Library/Application Support/spacehog` on macOS, `~/.local/share/spacehog` elsewhere.
pub fn data_dir() -> PathBuf {
    from_env("SPACEHOG_DATA_DIR")
        .unwrap_or_else(|| dirs::data_dir().unwrap_or_else(home).join("spacehog"))
}

/// Logs and the journal live here: `~/.local/state/spacehog` on every platform.
pub fn state_dir() -> PathBuf {
    from_env("SPACEHOG_STATE_DIR").unwrap_or_else(|| {
        from_env("XDG_STATE_HOME")
            .unwrap_or_else(|| home().join(".local/state"))
            .join("spacehog")
    })
}

/// `~/.config/spacehog` on every platform, matching the documented config path.
pub fn config_dir() -> PathBuf {
    from_env("SPACEHOG_CONFIG_DIR").unwrap_or_else(|| {
        from_env("XDG_CONFIG_HOME")
            .unwrap_or_else(|| home().join(".config"))
            .join("spacehog")
    })
}

pub fn db_path() -> PathBuf {
    from_env("SPACEHOG_DB").unwrap_or_else(|| data_dir().join("index.db"))
}

pub fn home_dir() -> PathBuf {
    home()
}
