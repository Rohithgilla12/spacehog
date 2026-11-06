# Decisions

Open questions from PRD section 14 and choices made while building, with the reason for each.

## Storage: SQLite, one `nodes` table, pre-order ids

SQLite (bundled, WAL) stays. Batched inserts in one transaction write about 450k rows per second on an M-series Mac, which is fast enough that `redb` is not worth losing ad-hoc SQL for agents and humans.

Node ids are assigned in pre-order, and every row stores `last_desc`, the id of its last descendant. A subtree is the id range `[id, last_desc]`, so "everything under this path" is a range scan with no recursive query and no stored path strings. Paths are rebuilt from `parent_id` on demand, and that cost is paid only for the rows a query returns.

## Walker: rayon recursion, not jwalk

The walker is a recursive `read_dir` + `lstat` over rayon's work-stealing pool. Each directory returns its subtree, which makes building the tree trivial. jwalk would hand back a flat stream that has to be stitched back into a tree. The walk is syscall-bound (about 14x more `sys` than `user` time), so the walker library is not the bottleneck either way.

## Older scans are compacted

The newest scan of a root keeps every node. Older scans (up to `keep_scans`, default 5) keep only directories, claimed items, and files of 10 MB or more. That is enough for `changes` and saves about 90% of rows. A disk-usage tool that stores five full copies of a 15M-file tree would be the hog.

## Fallback rules and exclusive sizes

The PRD says the first matching rule claims a subtree and stops descent. Catch-alls such as `~/Library/Application Support/*` and user folders such as `~/Documents` would then hide the rules underneath them: VS Code caches, `.git/lfs`, and `node_modules` in projects kept under `~/Documents`.

Rules can set `fallback = true`. A fallback claims its subtree and keeps descending, and deeper matches carve out nested claims. Each claim stores `excl_disk`, its size minus the claims nested in it, so bucket totals are a plain `SUM(excl_disk)` with no double counting. `tests/rules.rs::summary_accounts_for_every_byte_once` proves it.

## Projects bucket

A directory containing `.git` is claimed as a `project` fallback (protected). Without it, source trees land in `other`, which was 32% of a projects folder in testing. The PRD's acceptance bar is under 10%.

## atime is advisory only

atime is recorded but not used. APFS commonly runs without access-time updates, so staleness comes from mtimes: for a project artefact, the newest of the parent directory, its manifests and lockfiles, sibling files, and `.git/HEAD` and `.git/index`, searched up to six levels for the project root.

## `dist/` and `build/`

`js-dist` starts as `review`. It is promoted to `regenerable` only when a lockfile sits beside it and `package.json` defines a `build` script. Gradle `build/` next to `build.gradle*` is regenerable outright.

## Staleness is stored as a timestamp

Rows store `stale_ref`, the unix time of last activity, not a day count. Day counts are computed at query time, so a scan from last month still reports correct ages.

## Docker breakdown

Deferred to v1.5 as the PRD suggests. `Docker.raw` is reported with apparent and allocated sizes, since it is sparse.

## Index write: deferred index builds did not help

Measured on a 2M-node tree (2026-11-06): writing rows with the three node indexes live took 4.43 s. Dropping them for the insert and rebuilding afterwards took 4.47 s and 5.17 s, and a second scan into the same database rose to 10.2 s because the rebuild re-indexes every kept scan. The cost is per-row insert work (about 2.2 µs per row), not index maintenance, so the indexes stay live.

## Walker: getattrlistbulk on macOS

On macOS the walker lists each directory with `getattrlistbulk`, which returns names and attributes for a batch of entries per syscall, instead of `readdir` plus one `lstat` per entry. A filesystem without support falls back to the portable path.

Measured on `~/.rustup` (162,674 files, warm cache, interleaved runs, 2026-11-06): walk 1,861 ms and 629 ms before, 443 ms and 235 ms after. `sys` time roughly halved. Files, dirs, allocated bytes, and apparent bytes are identical between the two walkers and match `du -sk` exactly.

Two layout facts the man page does not make obvious, both covered by `bulk::tests`: the directory attribute group is packed only for directories and the file group only for non-directories, even with `FSOPT_PACK_INVAL_ATTRS`; and `ATTR_CMN_ERROR` sits immediately after the returned-attribute set, before the name.
