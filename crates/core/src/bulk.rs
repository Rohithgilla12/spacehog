//! Directory listing with attributes in one syscall per batch, via macOS
//! `getattrlistbulk`. It replaces a `readdir` plus one `lstat` per entry,
//! which is where a full-disk walk spends most of its time.

use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::model::NodeKind;

#[derive(Debug, Clone)]
pub struct BulkEntry {
    pub name: Box<str>,
    pub kind: NodeKind,
    pub apparent: u64,
    pub alloc: u64,
    pub mtime: i64,
    pub atime: i64,
    pub ino: u64,
    pub dev: u64,
    pub nlink: u64,
    pub uid: u32,
    pub st_flags: u32,
    pub errno: i32,
}

const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;
const BUF_SIZE: usize = 256 * 1024;

const COMMON: u32 = libc::ATTR_CMN_RETURNED_ATTRS
    | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_ACCTIME
    | libc::ATTR_CMN_OWNERID
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID
    | ATTR_CMN_ERROR;
const DIR: u32 = libc::ATTR_DIR_ALLOCSIZE | libc::ATTR_DIR_DATALENGTH;
const FILE: u32 = libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_TOTALSIZE | libc::ATTR_FILE_ALLOCSIZE;

/// Reads the packed record of one entry: attributes in bit order, 4-byte
/// aligned, with the name bytes stored after them.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let out: [u8; N] = self.buf[self.pos..self.pos + N]
            .try_into()
            .expect("in bounds");
        self.pos += N.next_multiple_of(4);
        out
    }

    fn u32(&mut self) -> u32 {
        u32::from_ne_bytes(self.take())
    }

    fn i32(&mut self) -> i32 {
        i32::from_ne_bytes(self.take())
    }

    fn u64(&mut self) -> u64 {
        u64::from_ne_bytes(self.take())
    }

    fn i64(&mut self) -> i64 {
        i64::from_ne_bytes(self.take())
    }

    fn timespec_secs(&mut self) -> i64 {
        let secs = self.i64();
        let _nanos = self.i64();
        secs
    }
}

struct Fd(libc::c_int);

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: the fd came from a successful `open` and is closed once.
        unsafe { libc::close(self.0) };
    }
}

/// Lists `dir` with attributes, skipping `.` and `..`. Fails with the OS
/// error when the directory cannot be opened or the filesystem lacks support.
pub fn read_dir(dir: &Path) -> std::io::Result<Vec<BulkEntry>> {
    let c = CString::new(dir.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    // SAFETY: `c` is a valid NUL-terminated path.
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = Fd(fd);
    let mut list = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: COMMON,
        volattr: 0,
        dirattr: DIR,
        fileattr: FILE,
        forkattr: 0,
    };
    let mut buf = vec![0u8; BUF_SIZE];
    let mut out = Vec::new();
    loop {
        // SAFETY: `list` and `buf` outlive the call and `buf.len()` is its capacity.
        let n = unsafe {
            libc::getattrlistbulk(
                fd.0,
                (&mut list as *mut libc::attrlist).cast(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                libc::FSOPT_PACK_INVAL_ATTRS as u64,
            )
        };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n == 0 {
            break;
        }
        let mut off = 0usize;
        for _ in 0..n {
            let len = u32::from_ne_bytes(buf[off..off + 4].try_into().expect("in bounds")) as usize;
            if let Some(e) = parse(&buf[off..off + len]) {
                out.push(e);
            }
            off += len;
        }
    }
    Ok(out)
}

fn parse(rec: &[u8]) -> Option<BulkEntry> {
    let mut c = Cursor { buf: rec, pos: 4 };
    let returned_common = c.u32();
    let _vol = c.u32();
    let _dir = c.u32();
    let _file = c.u32();
    let _fork = c.u32();
    // ATTR_CMN_ERROR is packed straight after the returned set, not in bit order.
    let error = c.u32();
    let name_ref_at = c.pos;
    let name_off = c.i32();
    let name_len = c.u32() as usize;
    let start = (name_ref_at as isize + name_off as isize) as usize;
    let raw = &rec[start..(start + name_len).min(rec.len())];
    let raw = raw.split(|&b| b == 0).next().unwrap_or_default();
    let name = OsStr::from_bytes(raw).to_string_lossy();
    if name == "." || name == ".." {
        return None;
    }
    let dev = c.i32() as u32 as u64;
    let objtype = c.u32();
    let mtime = c.timespec_secs();
    let atime = c.timespec_secs();
    let uid = c.u32();
    let st_flags = c.u32();
    let ino = c.u64();
    let errno = if returned_common & ATTR_CMN_ERROR != 0 {
        error as i32
    } else {
        0
    };
    let kind = match objtype {
        VREG => NodeKind::File,
        VDIR => NodeKind::Dir,
        VLNK => NodeKind::Symlink,
        _ => NodeKind::Other,
    };
    // The directory group is packed only for directories and the file group
    // only for everything else, whatever FSOPT_PACK_INVAL_ATTRS says.
    let (apparent, alloc, nlink) = if kind == NodeKind::Dir {
        let alloc = c.u64();
        let len = c.u64();
        (len, alloc, 1)
    } else {
        let nlink = c.u32() as u64;
        let len = c.i64().max(0) as u64;
        let alloc = c.i64().max(0) as u64;
        (len, alloc, nlink)
    };
    Some(BulkEntry {
        name: name.into(),
        kind,
        apparent,
        alloc,
        mtime,
        atime,
        ino,
        dev,
        nlink,
        uid,
        st_flags,
        errno,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn matches_lstat_for_files_dirs_symlinks_and_hardlinks() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("small"), b"hi").unwrap();
        std::fs::write(d.join("big"), vec![1u8; 300_000]).unwrap();
        std::fs::hard_link(d.join("big"), d.join("big-link")).unwrap();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/x"), b"x").unwrap();
        std::os::unix::fs::symlink("big", d.join("sym")).unwrap();
        std::fs::write(d.join("naïve name with spaces"), b"unicode").unwrap();

        let mut entries = super::read_dir(d).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.len(), 6);
        for e in &entries {
            let m = std::fs::symlink_metadata(d.join(&*e.name)).unwrap();
            assert_eq!(e.ino, m.ino(), "{}", e.name);
            assert_eq!(e.dev, m.dev(), "{}", e.name);
            assert_eq!(e.mtime, m.mtime(), "{}", e.name);
            assert_eq!(e.uid, m.uid(), "{}", e.name);
            if !m.is_dir() {
                assert_eq!(e.apparent, m.len(), "{}", e.name);
                assert_eq!(e.alloc, m.blocks() * 512, "{}", e.name);
                assert_eq!(e.nlink, m.nlink(), "{}", e.name);
            }
            let kind_ok = match e.kind {
                crate::model::NodeKind::File => m.file_type().is_file(),
                crate::model::NodeKind::Dir => m.is_dir(),
                crate::model::NodeKind::Symlink => m.file_type().is_symlink(),
                _ => false,
            };
            assert!(kind_ok, "{} kind", e.name);
        }
    }
}
