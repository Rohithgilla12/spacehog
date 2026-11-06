use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error("invalid rules: {0}")]
    Rules(String),
    #[error("invalid config: {0}")]
    Config(String),
    #[error("a scan is already running (pid {pid}, scan {scan_id})")]
    ScanInProgress { pid: u32, scan_id: i64 },
    #[error("no finished scan in the index; run `spacehog scan` first")]
    NoScan,
    #[error("path is not in the index: {}", .0.display())]
    NotIndexed(PathBuf),
    #[error("{0}")]
    Invalid(String),
    #[error("refused: {0}")]
    Refused(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
