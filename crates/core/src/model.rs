use serde::{Deserialize, Serialize};

pub type ScanId = i64;
pub type NodeId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    File,
    Dir,
    Symlink,
    Mount,
    Other,
}

impl NodeKind {
    pub fn as_i64(self) -> i64 {
        match self {
            NodeKind::File => 0,
            NodeKind::Dir => 1,
            NodeKind::Symlink => 2,
            NodeKind::Mount => 3,
            NodeKind::Other => 4,
        }
    }

    pub fn from_i64(v: i64) -> Self {
        match v {
            0 => NodeKind::File,
            1 => NodeKind::Dir,
            2 => NodeKind::Symlink,
            3 => NodeKind::Mount,
            _ => NodeKind::Other,
        }
    }

    pub fn is_container(self) -> bool {
        matches!(self, NodeKind::Dir)
    }
}

/// Bit flags stored per node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeFlags(pub u8);

impl NodeFlags {
    pub const ERROR: u8 = 1;
    pub const CLOUD_ONLY: u8 = 1 << 1;
    pub const MOUNT: u8 = 1 << 2;
    pub const EXCLUDED: u8 = 1 << 3;
    pub const DELETED: u8 = 1 << 4;

    pub fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    pub fn set(&mut self, bit: u8) {
        self.0 |= bit;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SizeMode {
    Disk,
    Apparent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRecord {
    pub id: ScanId,
    pub root: String,
    pub status: ScanStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub apparent_bytes: u64,
    pub errors: u64,
    pub current_path: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanStatus {
    Running,
    Done,
    Failed,
}

impl ScanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ScanStatus::Running => "running",
            ScanStatus::Done => "done",
            ScanStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "running" => ScanStatus::Running,
            "done" => ScanStatus::Done,
            _ => ScanStatus::Failed,
        }
    }
}

/// One row of the index, joined with its absolute path.
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub id: NodeId,
    pub scan_id: ScanId,
    pub parent_id: Option<NodeId>,
    pub path: String,
    pub name: String,
    pub kind: NodeKind,
    pub disk_size: u64,
    pub apparent_size: u64,
    pub file_count: u64,
    pub child_count: u64,
    pub mtime: i64,
    pub max_mtime: i64,
    pub atime: Option<i64>,
    pub inode: u64,
    pub dev: u64,
    pub nlink: u64,
    pub uid: u32,
    pub flags: NodeFlags,
    pub errno: Option<i32>,
    pub rule_id: Option<String>,
    pub bucket: Option<String>,
    pub stale_days: Option<i64>,
    pub project_root: Option<String>,
    pub claim: bool,
    pub excl_disk: Option<u64>,
    pub excl_apparent: Option<u64>,
    pub regen_hint: Option<String>,
    pub last_desc: NodeId,
    pub depth: u32,
    /// Safety recorded at scan time; config overrides apply on top at query time.
    pub safety: Option<crate::rules::Safety>,
}

impl NodeInfo {
    pub fn size(&self, mode: SizeMode) -> u64 {
        match mode {
            SizeMode::Disk => self.disk_size,
            SizeMode::Apparent => self.apparent_size,
        }
    }
}

/// Categorisation result for one node, aligned by index with a `ScanTree`.
#[derive(Debug, Clone, Default)]
pub struct Label {
    pub rule_id: Option<std::sync::Arc<str>>,
    pub bucket: Option<std::sync::Arc<str>>,
    /// True for the node whose rule claimed the subtree.
    pub claim: bool,
    /// Unix time of the last activity that staleness is measured from.
    pub stale_ref: Option<i64>,
    pub project_root: Option<std::sync::Arc<str>>,
    /// Size of a claim minus the claims nested inside it.
    pub excl_disk: u64,
    pub excl_apparent: u64,
    pub regen_hint: Option<std::sync::Arc<str>>,
    pub safety: Option<crate::rules::Safety>,
}

/// Labels for a whole tree. Most nodes share an inherited label with the rest
/// of their claim, so nodes index into a palette instead of owning a copy.
#[derive(Debug, Clone, Default)]
pub struct Labels {
    pub palette: Vec<Label>,
    pub refs: Vec<u32>,
}

impl Labels {
    pub fn new(nodes: usize) -> Self {
        Self {
            palette: vec![Label::default()],
            refs: vec![0; nodes],
        }
    }

    pub fn push(&mut self, label: Label) -> u32 {
        self.palette.push(label);
        self.palette.len() as u32 - 1
    }

    pub fn get(&self, node: usize) -> &Label {
        &self.palette[self.refs[node] as usize]
    }

    /// Mutable access to a node's label. Only claims own a unique palette
    /// entry, so this is for claim nodes.
    pub fn get_mut(&mut self, node: usize) -> &mut Label {
        debug_assert!(self.get(node).claim, "only claim labels are unique");
        let i = self.refs[node] as usize;
        &mut self.palette[i]
    }
}
