use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::error::Result;
use crate::model::{
    Label, Labels, NodeFlags, NodeId, NodeInfo, NodeKind, ScanId, ScanRecord, ScanStatus,
};
use crate::scan::{ProgressSnapshot, ScanTree};

const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE scans (
    id             INTEGER PRIMARY KEY,
    root           TEXT    NOT NULL,
    status         TEXT    NOT NULL,
    pid            INTEGER,
    started_at     INTEGER NOT NULL,
    finished_at    INTEGER,
    files          INTEGER NOT NULL DEFAULT 0,
    dirs           INTEGER NOT NULL DEFAULT 0,
    bytes          INTEGER NOT NULL DEFAULT 0,
    apparent_bytes INTEGER NOT NULL DEFAULT 0,
    errors         INTEGER NOT NULL DEFAULT 0,
    current_path   TEXT,
    root_node      INTEGER,
    compacted      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX scans_root ON scans (root, finished_at DESC);

CREATE TABLE nodes (
    id           INTEGER PRIMARY KEY,
    scan_id      INTEGER NOT NULL,
    parent_id    INTEGER,
    last_desc    INTEGER NOT NULL,
    depth        INTEGER NOT NULL,
    name         TEXT    NOT NULL,
    kind         INTEGER NOT NULL,
    disk         INTEGER NOT NULL,
    apparent     INTEGER NOT NULL,
    file_count   INTEGER NOT NULL,
    child_count  INTEGER NOT NULL,
    mtime        INTEGER NOT NULL,
    max_mtime    INTEGER NOT NULL,
    atime        INTEGER,
    inode        INTEGER NOT NULL,
    dev          INTEGER NOT NULL,
    nlink        INTEGER NOT NULL,
    uid          INTEGER NOT NULL,
    flags        INTEGER NOT NULL,
    errno        INTEGER,
    rule_id      TEXT,
    bucket       TEXT,
    claim        INTEGER NOT NULL DEFAULT 0,
    excl_disk    INTEGER,
    excl_apparent INTEGER,
    stale_ref    INTEGER,
    project_root TEXT,
    regen_hint   TEXT,
    safety       TEXT
);
CREATE INDEX nodes_parent ON nodes (parent_id, disk DESC);
CREATE INDEX nodes_scan_disk ON nodes (scan_id, disk DESC);
CREATE INDEX nodes_claims ON nodes (scan_id, excl_disk DESC) WHERE claim = 1;

CREATE TABLE proposals (
    id          TEXT    PRIMARY KEY,
    scan_id     INTEGER,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    agent       TEXT    NOT NULL,
    rationale   TEXT    NOT NULL,
    mode        TEXT    NOT NULL,
    status      TEXT    NOT NULL,
    total_bytes INTEGER NOT NULL,
    total_human TEXT    NOT NULL,
    reason      TEXT,
    journal_id  TEXT
);
CREATE INDEX proposals_status ON proposals (status, created_at DESC);

CREATE TABLE proposal_items (
    proposal_id TEXT    NOT NULL,
    path        TEXT    NOT NULL,
    node_id     INTEGER,
    size        INTEGER NOT NULL,
    inode       INTEGER NOT NULL,
    dev         INTEGER NOT NULL,
    kind        INTEGER NOT NULL,
    bucket      TEXT,
    rule_id     TEXT,
    safety      TEXT    NOT NULL,
    stale_days  INTEGER,
    regen_hint  TEXT,
    PRIMARY KEY (proposal_id, path)
);

CREATE TABLE journal (
    id          INTEGER PRIMARY KEY,
    journal_id  TEXT    NOT NULL,
    proposal_id TEXT,
    agent       TEXT,
    path        TEXT    NOT NULL,
    size        INTEGER NOT NULL,
    inode       INTEGER NOT NULL,
    mode        TEXT    NOT NULL,
    outcome     TEXT    NOT NULL,
    error       TEXT,
    ts          INTEGER NOT NULL
);
CREATE INDEX journal_by_id ON journal (journal_id);
"#,
    r#"
ALTER TABLE journal ADD COLUMN trashed_to TEXT;
"#,
    r#"
ALTER TABLE scans ADD COLUMN phase TEXT;
ALTER TABLE scans ADD COLUMN written INTEGER NOT NULL DEFAULT 0;
"#,
];

/// Older scans of a root keep only directories, claimed items, and files at
/// least this large, which is enough for `changes` without storing every file N times.
pub(crate) const COMPACT_MIN_FILE: i64 = 10 * 1024 * 1024;

pub struct Index {
    pub(crate) conn: Connection,
}

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

impl Index {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut index = Self { conn };
        index.migrate()?;
        Ok(index)
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&crate::paths::db_path())
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn begin_scan(&self, root: &Path) -> Result<ScanId> {
        self.conn.execute(
            "INSERT INTO scans (root, status, pid, started_at) VALUES (?1, 'running', ?2, ?3)",
            params![root.to_string_lossy(), std::process::id(), now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_progress(&self, scan: ScanId, p: &ProgressSnapshot) -> Result<()> {
        self.conn.execute(
            "UPDATE scans SET files = ?2, dirs = ?3, bytes = ?4, errors = ?5, current_path = ?6,
                phase = ?7, written = ?8
             WHERE id = ?1 AND status = 'running'",
            params![
                scan,
                p.files as i64,
                p.dirs as i64,
                p.bytes as i64,
                p.errors as i64,
                p.current_path,
                serde_json::to_value(p.phase)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned)),
                p.written as i64,
            ],
        )?;
        Ok(())
    }

    pub fn fail_scan(&self, scan: ScanId, message: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE scans SET status = 'failed', finished_at = ?2, current_path = ?3 WHERE id = ?1",
            params![scan, now(), message],
        )?;
        Ok(())
    }

    /// Writes every node in one transaction and marks the scan done.
    pub fn finish_scan(
        &mut self,
        scan: ScanId,
        tree: &ScanTree,
        labels: Option<&Labels>,
        keep: usize,
        progress: Option<&crate::scan::Progress>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let base: i64 = tx.query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM nodes", [], |r| {
            r.get(0)
        })?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO nodes (id, scan_id, parent_id, last_desc, depth, name, kind, disk,
                    apparent, file_count, child_count, mtime, max_mtime, atime, inode, dev, nlink,
                    uid, flags, errno, rule_id, bucket, claim, stale_ref, project_root, excl_disk,
                    excl_apparent, regen_hint, safety)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                    ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29)",
            )?;
            let empty = Label::default();
            for (i, n) in tree.nodes.iter().enumerate() {
                if let Some(p) = progress.filter(|_| i % 65_536 == 0) {
                    p.written
                        .store(i as u64, std::sync::atomic::Ordering::Relaxed);
                }
                let label = labels.map_or(&empty, |l| l.get(i));
                stmt.execute(params![
                    base + i as i64,
                    scan,
                    n.parent.map(|p| base + p as i64),
                    base + n.last_desc as i64,
                    n.depth,
                    n.name.as_ref(),
                    n.kind.as_i64(),
                    n.disk as i64,
                    n.apparent as i64,
                    n.file_count as i64,
                    n.child_count,
                    n.mtime,
                    n.max_mtime,
                    n.atime,
                    n.ino as i64,
                    n.dev as i64,
                    n.nlink as i64,
                    n.uid,
                    n.flags.0,
                    (n.errno != 0).then_some(n.errno),
                    label.rule_id.as_deref(),
                    label.bucket.as_deref(),
                    label.claim,
                    label.stale_ref,
                    label.project_root.as_deref(),
                    label.claim.then_some(label.excl_disk as i64),
                    label.claim.then_some(label.excl_apparent as i64),
                    label.regen_hint.as_deref(),
                    label.safety.map(|s| s.as_str()),
                ])?;
            }
        }
        let (files, dirs, disk, apparent) = tree.totals();
        let errors = tree
            .nodes
            .iter()
            .filter(|n| n.flags.has(NodeFlags::ERROR))
            .count();
        tx.execute(
            "UPDATE scans SET status = 'done', finished_at = ?2, files = ?3, dirs = ?4, bytes = ?5,
                apparent_bytes = ?6, errors = ?7, current_path = NULL, root_node = ?8
             WHERE id = ?1",
            params![
                scan,
                now(),
                files as i64,
                dirs as i64,
                disk as i64,
                apparent as i64,
                errors as i64,
                base
            ],
        )?;
        tx.commit()?;
        self.prune(&tree.root, keep)?;
        Ok(())
    }

    /// Compacts every finished scan of `root` except the newest, and drops
    /// scans beyond `keep`.
    fn prune(&mut self, root: &Path, keep: usize) -> Result<()> {
        let root = root.to_string_lossy();
        let ids: Vec<(ScanId, bool)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, compacted FROM scans WHERE root = ?1 AND status = 'done'
                 ORDER BY finished_at DESC, id DESC",
            )?;
            stmt.query_map([&root], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        let tx = self.conn.transaction()?;
        for (i, (id, compacted)) in ids.into_iter().enumerate() {
            if i >= keep.max(1) {
                tx.execute("DELETE FROM nodes WHERE scan_id = ?1", [id])?;
                tx.execute("DELETE FROM scans WHERE id = ?1", [id])?;
            } else if i > 0 && !compacted {
                tx.execute(
                    "DELETE FROM nodes WHERE scan_id = ?1 AND kind != ?2 AND claim = 0 AND disk < ?3",
                    params![id, NodeKind::Dir.as_i64(), COMPACT_MIN_FILE],
                )?;
                tx.execute("UPDATE scans SET compacted = 1 WHERE id = ?1", [id])?;
            }
        }
        tx.execute(
            "DELETE FROM scans WHERE status = 'failed' AND root = ?1 AND finished_at < ?2",
            params![root, now() - 86_400],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn scan(&self, id: ScanId) -> Result<Option<ScanRecord>> {
        Ok(self
            .conn
            .query_row(&format!("{SCAN_SELECT} WHERE id = ?1"), [id], scan_from_row)
            .optional()?)
    }

    /// The newest finished scan, optionally restricted to a root.
    pub fn latest_scan(&self, root: Option<&Path>) -> Result<Option<ScanRecord>> {
        let row = match root {
            Some(root) => self.conn.query_row(
                &format!(
                    "{SCAN_SELECT} WHERE status = 'done' AND root = ?1 ORDER BY finished_at DESC, id DESC LIMIT 1"
                ),
                [root.to_string_lossy()],
                scan_from_row,
            ),
            None => self.conn.query_row(
                &format!("{SCAN_SELECT} WHERE status = 'done' ORDER BY finished_at DESC, id DESC LIMIT 1"),
                [],
                scan_from_row,
            ),
        };
        Ok(row.optional()?)
    }

    /// Finished scans of a root, newest first.
    pub fn scans_for_root(&self, root: &str) -> Result<Vec<ScanRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SCAN_SELECT} WHERE status = 'done' AND root = ?1 ORDER BY finished_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map([root], scan_from_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// A running scan whose process is still alive. Dead runners are marked failed.
    pub fn scan_in_progress(&self) -> Result<Option<ScanRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SCAN_SELECT} WHERE status = 'running' ORDER BY id DESC"
        ))?;
        let running: Vec<ScanRecord> = stmt
            .query_map([], scan_from_row)?
            .collect::<std::result::Result<_, _>>()?;
        for scan in running {
            if scan.pid.is_some_and(crate::lock::pid_alive) {
                return Ok(Some(scan));
            }
            self.fail_scan(scan.id, "scanner exited")?;
        }
        Ok(None)
    }

    pub fn root_node(&self, scan: ScanId) -> Result<Option<NodeInfo>> {
        let id: Option<Option<NodeId>> = self
            .conn
            .query_row("SELECT root_node FROM scans WHERE id = ?1", [scan], |r| {
                r.get(0)
            })
            .optional()?;
        match id.flatten() {
            Some(id) => self.node(id),
            None => Ok(None),
        }
    }

    pub fn node(&self, id: NodeId) -> Result<Option<NodeInfo>> {
        let row = self
            .conn
            .query_row(&format!("{NODE_SELECT} WHERE id = ?1"), [id], node_from_row)
            .optional()?;
        row.map(|n| self.with_path(n)).transpose()
    }

    /// Children of a node, largest first.
    pub fn children(&self, id: NodeId, limit: usize) -> Result<Vec<NodeInfo>> {
        let parent = match self.node(id)? {
            Some(p) => p,
            None => return Ok(Vec::new()),
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "{NODE_SELECT} WHERE parent_id = ?1 AND (flags & {deleted}) = 0 ORDER BY disk DESC LIMIT ?2",
            deleted = NodeFlags::DELETED
        ))?;
        let rows = stmt.query_map(params![id, limit as i64], node_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            let mut n = row?;
            n.path = join(&parent.path, &n.name);
            out.push(n);
        }
        Ok(out)
    }

    /// Resolves an absolute path to its node in a scan.
    pub fn find(&self, scan: ScanId, path: &Path) -> Result<Option<NodeInfo>> {
        let Some(root) = self.root_node(scan)? else {
            return Ok(None);
        };
        let Ok(rest) = path.strip_prefix(&root.path) else {
            return Ok(None);
        };
        let mut id = root.id;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM nodes WHERE parent_id = ?1 AND name = ?2")?;
        for comp in rest.components() {
            let Component::Normal(name) = comp else {
                continue;
            };
            match stmt
                .query_row(params![id, name.to_string_lossy()], |r| r.get(0))
                .optional()?
            {
                Some(next) => id = next,
                None => return Ok(None),
            }
        }
        self.node(id)
    }

    pub fn path_of(&self, id: NodeId) -> Result<String> {
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE up(id, parent_id, name, d) AS (
                SELECT id, parent_id, name, 0 FROM nodes WHERE id = ?1
                UNION ALL
                SELECT n.id, n.parent_id, n.name, up.d + 1 FROM nodes n JOIN up ON n.id = up.parent_id
             ) SELECT name FROM up ORDER BY d DESC",
        )?;
        let names: Vec<String> = stmt
            .query_map([id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut path = PathBuf::new();
        for n in names {
            path.push(n);
        }
        Ok(path.to_string_lossy().into_owned())
    }

    pub(crate) fn with_path(&self, mut n: NodeInfo) -> Result<NodeInfo> {
        n.path = self.path_of(n.id)?;
        Ok(n)
    }

    /// Runs a query built from `NODE_SELECT` and fills in paths.
    pub(crate) fn nodes_where(
        &self,
        clause: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<NodeInfo>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{NODE_SELECT} {clause}"))?;
        let rows: Vec<NodeInfo> = stmt
            .query_map(params, node_from_row)?
            .collect::<std::result::Result<_, _>>()?;
        rows.into_iter().map(|n| self.with_path(n)).collect()
    }

    /// Flags a subtree deleted and takes its bytes out of every total above it.
    pub fn mark_deleted(&self, id: NodeId) -> Result<()> {
        self.adjust_deleted(id, true)
    }

    /// Reverses `mark_deleted` after an undo.
    pub fn mark_restored(&self, id: NodeId) -> Result<()> {
        self.adjust_deleted(id, false)
    }

    fn adjust_deleted(&self, id: NodeId, deleting: bool) -> Result<()> {
        let Some(n) = self.node(id)? else {
            return Ok(());
        };
        if n.flags.has(NodeFlags::DELETED) == deleting {
            return Ok(());
        }
        let sign: i64 = if deleting { -1 } else { 1 };
        let (disk, apparent, files) = (
            sign * n.disk_size as i64,
            sign * n.apparent_size as i64,
            sign * n.file_count as i64,
        );
        let tx = self.conn.unchecked_transaction()?;
        let flag_sql = if deleting {
            "UPDATE nodes SET flags = flags | ?3 WHERE id BETWEEN ?1 AND ?2"
        } else {
            "UPDATE nodes SET flags = flags & ~?3 WHERE id BETWEEN ?1 AND ?2"
        };
        tx.execute(flag_sql, params![id, n.last_desc, NodeFlags::DELETED])?;
        let nested_claims: i64 = tx.query_row(
            "SELECT COALESCE(SUM(excl_disk), 0) FROM nodes WHERE claim = 1 AND id > ?1 AND id <= ?2",
            params![id, n.last_desc],
            |r| r.get(0),
        )?;
        let mut owner_adjusted = n.claim;
        let mut cur = n.parent_id;
        while let Some(p) = cur {
            let (parent, is_claim): (Option<NodeId>, bool) = tx.query_row(
                "SELECT parent_id, claim FROM nodes WHERE id = ?1",
                [p],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            tx.execute(
                "UPDATE nodes SET disk = MAX(disk + ?2, 0), apparent = MAX(apparent + ?3, 0),
                    file_count = MAX(file_count + ?4, 0) WHERE id = ?1",
                params![p, disk, apparent, files],
            )?;
            if is_claim && !owner_adjusted {
                tx.execute(
                    "UPDATE nodes SET excl_disk = MAX(excl_disk + ?2, 0) WHERE id = ?1",
                    params![p, disk - sign * nested_claims],
                )?;
                owner_adjusted = true;
            }
            cur = parent;
        }
        tx.execute(
            "UPDATE scans SET bytes = MAX(bytes + ?2, 0), apparent_bytes = MAX(apparent_bytes + ?3, 0)
             WHERE id = ?1",
            params![n.scan_id, disk, apparent],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn is_compacted(&self, scan: ScanId) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT compacted FROM scans WHERE id = ?1", [scan], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or(false))
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

pub(crate) fn join(parent: &str, name: &str) -> String {
    Path::new(parent).join(name).to_string_lossy().into_owned()
}

const SCAN_SELECT: &str = "SELECT id, root, status, started_at, finished_at, files, dirs, bytes,
    apparent_bytes, errors, current_path, pid, phase, written FROM scans";

fn scan_from_row(r: &Row<'_>) -> rusqlite::Result<ScanRecord> {
    Ok(ScanRecord {
        id: r.get(0)?,
        root: r.get(1)?,
        status: ScanStatus::parse(&r.get::<_, String>(2)?),
        started_at: r.get(3)?,
        finished_at: r.get(4)?,
        files: r.get::<_, i64>(5)? as u64,
        dirs: r.get::<_, i64>(6)? as u64,
        bytes: r.get::<_, i64>(7)? as u64,
        apparent_bytes: r.get::<_, i64>(8)? as u64,
        errors: r.get::<_, i64>(9)? as u64,
        current_path: r.get(10)?,
        pid: r.get(11)?,
        phase: r.get(12)?,
        written: r.get::<_, i64>(13)? as u64,
    })
}

pub(crate) const NODE_SELECT: &str = "SELECT id, scan_id, parent_id, name, kind, disk, apparent,
    file_count, child_count, mtime, max_mtime, atime, inode, dev, nlink, uid, flags, errno,
    rule_id, bucket, stale_ref, project_root, claim, excl_disk, excl_apparent, regen_hint,
    last_desc, depth, safety FROM nodes";

pub(crate) fn node_from_row(r: &Row<'_>) -> rusqlite::Result<NodeInfo> {
    Ok(NodeInfo {
        id: r.get(0)?,
        scan_id: r.get(1)?,
        parent_id: r.get(2)?,
        path: String::new(),
        name: r.get(3)?,
        kind: NodeKind::from_i64(r.get(4)?),
        disk_size: r.get::<_, i64>(5)? as u64,
        apparent_size: r.get::<_, i64>(6)? as u64,
        file_count: r.get::<_, i64>(7)? as u64,
        child_count: r.get::<_, i64>(8)? as u64,
        mtime: r.get(9)?,
        max_mtime: r.get(10)?,
        atime: r.get(11)?,
        inode: r.get::<_, i64>(12)? as u64,
        dev: r.get::<_, i64>(13)? as u64,
        nlink: r.get::<_, i64>(14)? as u64,
        uid: r.get(15)?,
        flags: NodeFlags(r.get(16)?),
        errno: r.get(17)?,
        rule_id: r.get(18)?,
        bucket: r.get(19)?,
        stale_days: r
            .get::<_, Option<i64>>(20)?
            .map(|t| (now() - t).max(0) / 86_400),
        project_root: r.get(21)?,
        claim: r.get(22)?,
        excl_disk: r.get::<_, Option<i64>>(23)?.map(|v| v as u64),
        excl_apparent: r.get::<_, Option<i64>>(24)?.map(|v| v as u64),
        regen_hint: r.get(25)?,
        last_desc: r.get(26)?,
        depth: r.get(27)?,
        safety: r
            .get::<_, Option<String>>(28)?
            .and_then(|s| crate::rules::Safety::parse(&s)),
    })
}
