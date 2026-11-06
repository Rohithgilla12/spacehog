use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Advisory scan lock: a file holding the owner's PID and scan id.
/// A lock whose PID is dead is treated as stale and taken over.
#[derive(Debug)]
pub struct ScanLock {
    path: PathBuf,
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    // SAFETY: kill with signal 0 only checks for existence and permission.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

impl ScanLock {
    pub fn acquire(dir: &Path, scan_id: i64) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join("scans.lock");
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    writeln!(f, "{} {}", std::process::id(), scan_id)?;
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Some((pid, held)) = read_lock(&path)
                        && pid_alive(pid)
                    {
                        return Err(Error::ScanInProgress { pid, scan_id: held });
                    }
                    let _ = fs::remove_file(&path);
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::Invalid(format!("could not take {}", path.display())))
    }
}

fn read_lock(path: &Path) -> Option<(u32, i64)> {
    let s = fs::read_to_string(path).ok()?;
    let mut it = s.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

impl Drop for ScanLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
