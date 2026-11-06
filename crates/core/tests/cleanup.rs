mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{home, scan, write};
use spacehog_core::Index;
use spacehog_core::cleanup::{ExecuteRequest, Outcome, Trash};
use spacehog_core::config::{Config, DeleteMode};
use spacehog_core::proposals::{Actor, ProposalStatus, ProposeRequest};
use spacehog_core::report;
use spacehog_core::rules::{DEFAULT_RULES, RuleSet, Safety};

struct World {
    _home_dir: tempfile::TempDir,
    _state: tempfile::TempDir,
    trash_dir: tempfile::TempDir,
    h: PathBuf,
    index: Index,
    rules: std::sync::Arc<RuleSet>,
}

impl World {
    fn new() -> Self {
        let (home_dir, h) = home();
        let state = tempfile::tempdir().unwrap();
        let (index, rules, _) = scan(&h, state.path());
        World {
            _home_dir: home_dir,
            _state: state,
            trash_dir: tempfile::tempdir().unwrap(),
            h,
            index,
            rules,
        }
    }

    fn trash(&self) -> Trash {
        Trash::Rename(self.trash_dir.path().to_path_buf())
    }

    fn propose(
        &self,
        config: &Config,
        actor: Actor,
        paths: &[&str],
        mode: Option<DeleteMode>,
    ) -> (String, String) {
        let r = self
            .index
            .propose(
                &self.rules,
                config,
                actor,
                &ProposeRequest {
                    paths: paths.iter().map(|p| self.h.join(p)).collect(),
                    rationale: "test".into(),
                    agent: Some("tester".into()),
                    mode,
                },
            )
            .unwrap();
        assert!(
            r.rejected.is_empty(),
            "unexpected rejections {:?}",
            r.rejected
        );
        (r.proposal_id.unwrap(), r.total_human)
    }

    fn execute(
        &self,
        config: &Config,
        actor: Actor,
        id: &str,
        confirm: &str,
    ) -> spacehog_core::Result<spacehog_core::cleanup::ExecutionReport> {
        self.index.execute(
            &self.rules,
            config,
            &self.trash(),
            &ExecuteRequest {
                proposal_id: id,
                confirm,
                actor,
            },
        )
    }

    fn rejected_reason(&self, actor: Actor, path: &Path) -> String {
        let r = self
            .index
            .propose(
                &self.rules,
                &Config::default(),
                actor,
                &ProposeRequest {
                    paths: vec![path.to_path_buf()],
                    rationale: "test".into(),
                    agent: None,
                    mode: None,
                },
            )
            .unwrap();
        assert!(r.accepted.is_empty(), "{} was accepted", path.display());
        r.rejected[0].reason.clone()
    }
}

#[test]
fn approved_trash_cleanup_moves_items_and_updates_the_index() {
    let w = World::new();
    let before =
        report::summary(&w.index, &w.rules, w.index.require_scan(None).unwrap(), 90).unwrap();
    let (id, total) = w.propose(
        &Config::default(),
        Actor::Agent,
        &["code/web/node_modules", ".npm/_cacache"],
        None,
    );
    w.index.approve(&id).unwrap();
    let report = w
        .execute(&Config::default(), Actor::Agent, &id, &total)
        .unwrap();

    assert_eq!(report.status, ProposalStatus::Executed);
    assert!(
        report.results.iter().all(|r| r.outcome == Outcome::Trashed),
        "{:?}",
        report.results
    );
    assert!(!w.h.join("code/web/node_modules").exists());
    assert!(
        w.trash_dir
            .path()
            .join("node_modules/react/index.js")
            .exists(),
        "lands in the trash intact"
    );
    assert_eq!(
        w.index.journal(&report.journal_id).unwrap().len(),
        2,
        "every item is journaled"
    );

    let after =
        report::summary(&w.index, &w.rules, w.index.require_scan(None).unwrap(), 90).unwrap();
    assert_eq!(
        after.total + report.bytes_freed,
        before.total,
        "index drops freed bytes without a rescan"
    );
    assert_eq!(
        after.buckets.iter().map(|b| b.bytes).sum::<u64>(),
        after.total
    );
}

#[test]
fn undo_restores_trashed_items_and_their_bytes() {
    let w = World::new();
    let scan_id = w.index.require_scan(None).unwrap();
    let before = report::summary(&w.index, &w.rules, scan_id, 90)
        .unwrap()
        .total;
    let (id, total) = w.propose(&Config::default(), Actor::User, &["code/rs/target"], None);
    let report = w
        .execute(&Config::default(), Actor::User, &id, &total)
        .unwrap();
    assert!(!w.h.join("code/rs/target").exists());

    let restored = w.index.undo(&report.journal_id).unwrap();
    assert_eq!(restored[0].outcome, Outcome::Restored);
    assert!(w.h.join("code/rs/target/debug/app").exists());
    assert_eq!(
        report::summary(&w.index, &w.rules, scan_id, 90)
            .unwrap()
            .total,
        before
    );
}

#[test]
fn agents_cannot_execute_pending_proposals_without_auto_approve() {
    let w = World::new();
    let (id, total) = w.propose(&Config::default(), Actor::Agent, &[".npm/_cacache"], None);
    let err = w
        .execute(&Config::default(), Actor::Agent, &id, &total)
        .unwrap_err();
    assert!(err.to_string().contains("pending approval"), "{err}");
    assert!(w.h.join(".npm/_cacache").exists());

    let mut auto = Config::default();
    auto.mcp.auto_approve = vec![Safety::Cache];
    assert_eq!(
        w.execute(&auto, Actor::Agent, &id, &total).unwrap().status,
        ProposalStatus::Executed
    );
}

#[test]
fn permanent_deletes_are_never_auto_approved() {
    let w = World::new();
    let mut auto = Config::default();
    auto.mcp.auto_approve = vec![Safety::Regenerable, Safety::Cache];
    let (id, total) = w.propose(
        &auto,
        Actor::Agent,
        &[".npm/_cacache"],
        Some(DeleteMode::Permanent),
    );
    let err = w.execute(&auto, Actor::Agent, &id, &total).unwrap_err();
    assert!(err.to_string().contains("pending approval"), "{err}");
    w.index.approve(&id).unwrap();
    let report = w.execute(&auto, Actor::Agent, &id, &total).unwrap();
    assert_eq!(report.results[0].outcome, Outcome::Deleted);
    assert!(
        fs::read_dir(w.trash_dir.path()).unwrap().next().is_none(),
        "permanent mode bypasses the trash"
    );
}

#[test]
fn confirm_string_must_match_the_total() {
    let w = World::new();
    let (id, _total) = w.propose(&Config::default(), Actor::Agent, &[".npm/_cacache"], None);
    w.index.approve(&id).unwrap();
    let err = w
        .execute(&Config::default(), Actor::Agent, &id, "yes")
        .unwrap_err();
    assert!(err.to_string().contains("confirm must equal"), "{err}");
}

#[test]
fn protected_paths_are_refused_at_proposal_and_at_execution() {
    let w = World::new();
    for p in [
        "code/web/.git",
        "Documents/essay.txt",
        "Library/Mail",
        "Applications/Tool.app",
        "code/web",
        "loose.png",
    ] {
        let reason = w.rejected_reason(Actor::User, &w.h.join(p));
        assert!(
            reason.starts_with("protected") || reason.contains(".git"),
            "{p}: {reason}"
        );
    }

    let (id, total) = w.propose(
        &Config::default(),
        Actor::User,
        &["code/web/node_modules"],
        None,
    );
    let user_rules = format!(
        "protect = [\"{}\"]\n",
        w.h.join("code/web/node_modules").display()
    );
    let stricter = RuleSet::from_sources(&[&user_rules, DEFAULT_RULES], &w.h).unwrap();
    let report = w
        .index
        .execute(
            &stricter,
            &Config::default(),
            &w.trash(),
            &ExecuteRequest {
                proposal_id: &id,
                confirm: &total,
                actor: Actor::User,
            },
        )
        .unwrap();
    assert_eq!(report.results[0].outcome, Outcome::Skipped);
    assert!(
        report.results[0]
            .reason
            .as_deref()
            .unwrap()
            .starts_with("protected")
    );
    assert!(
        w.h.join("code/web/node_modules").exists(),
        "protection added after proposing still holds"
    );
    assert_eq!(report.status, ProposalStatus::Failed);
}

#[test]
fn home_root_ancestors_and_unindexed_paths_are_refused() {
    let w = World::new();
    assert!(w.rejected_reason(Actor::User, &w.h).contains("scan root"));
    assert!(
        w.rejected_reason(Actor::User, w.h.parent().unwrap())
            .contains("not in the index")
    );
    let outside = tempfile::tempdir().unwrap();
    assert!(
        w.rejected_reason(Actor::User, outside.path())
            .contains("not in the index")
    );
    assert!(
        w.rejected_reason(Actor::User, &w.h.join("code/web/node_modules/../../web"))
            .contains(".git")
            || w.rejected_reason(Actor::User, &w.h.join("code/web"))
                .starts_with("protected")
    );
}

#[test]
fn a_directory_holding_a_git_repo_is_never_treated_as_a_cache() {
    let (home_dir, h) = home();
    write(&h.join("code/web/node_modules/forked/.git/HEAD"), 20);
    write(&h.join("code/web/node_modules/forked/index.js"), 100);
    let state = tempfile::tempdir().unwrap();
    let (index, rules, _) = scan(&h, state.path());
    let r = index
        .propose(
            &rules,
            &Config::default(),
            Actor::User,
            &ProposeRequest {
                paths: vec![h.join("code/web/node_modules/forked")],
                rationale: "t".into(),
                agent: None,
                mode: None,
            },
        )
        .unwrap();
    assert!(r.rejected[0].reason.contains(".git"), "{:?}", r.rejected);
    drop(home_dir);
}

#[test]
fn items_changed_since_indexing_are_skipped() {
    let w = World::new();
    let (id, total) = w.propose(
        &Config::default(),
        Actor::User,
        &["code/web/node_modules", "code/rs/target"],
        None,
    );
    write(&w.h.join("code/web/node_modules/react/new.js"), 10);
    let target = w.h.join("code/rs/target");
    fs::rename(&target, w.h.join("code/rs/target-old")).unwrap();
    fs::create_dir_all(target.join("debug")).unwrap();
    fs::write(target.join("debug/app"), vec![0u8; 9000]).unwrap();

    let report = w
        .execute(&Config::default(), Actor::User, &id, &total)
        .unwrap();
    let reasons: Vec<_> = report
        .results
        .iter()
        .map(|r| (r.outcome, r.reason.clone().unwrap_or_default()))
        .collect();
    assert!(
        reasons.iter().all(|(o, _)| *o == Outcome::Skipped),
        "{reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|(_, r)| r.contains("changed since it was indexed")),
        "{reasons:?}"
    );
    assert!(
        reasons.iter().any(|(_, r)| r.contains("inode changed")),
        "{reasons:?}"
    );
    assert!(w.h.join("code/web/node_modules/react/index.js").exists());
}

#[test]
fn symlinks_delete_the_link_never_the_target() {
    let (home_dir, h) = home();
    write(&h.join("precious/data.bin"), 5000);
    std::os::unix::fs::symlink(h.join("precious"), h.join("code/web/node_modules/linked")).unwrap();
    let state = tempfile::tempdir().unwrap();
    let (index, rules, _) = scan(&h, state.path());
    let trash = tempfile::tempdir().unwrap();
    let r = index
        .propose(
            &rules,
            &Config::default(),
            Actor::User,
            &ProposeRequest {
                paths: vec![h.join("code/web/node_modules/linked")],
                rationale: "t".into(),
                agent: None,
                mode: Some(DeleteMode::Permanent),
            },
        )
        .unwrap();
    assert_eq!(r.accepted.len(), 1, "{:?}", r.rejected);
    assert_eq!(
        r.accepted[0].path,
        h.join("code/web/node_modules/linked").to_string_lossy(),
        "link is not resolved"
    );
    let id = r.proposal_id.unwrap();
    let report = index
        .execute(
            &rules,
            &Config::default(),
            &Trash::Rename(trash.path().to_path_buf()),
            &ExecuteRequest {
                proposal_id: &id,
                confirm: &r.total_human,
                actor: Actor::User,
            },
        )
        .unwrap();
    assert_eq!(report.results[0].outcome, Outcome::Deleted);
    assert!(fs::symlink_metadata(h.join("code/web/node_modules/linked")).is_err());
    assert!(h.join("precious/data.bin").exists(), "target untouched");
    drop(home_dir);
}

#[test]
fn caps_tightened_after_proposing_still_apply() {
    let w = World::new();
    let (id, total) = w.propose(
        &Config::default(),
        Actor::User,
        &["code/web/node_modules", ".npm/_cacache"],
        None,
    );
    let mut tight = Config::default();
    tight.cleanup.max_items = 1;
    assert!(
        w.execute(&tight, Actor::User, &id, &total)
            .unwrap_err()
            .to_string()
            .contains("max_items")
    );
    let mut small = Config::default();
    small.cleanup.max_bytes_per_execution = 10;
    assert!(
        w.execute(&small, Actor::User, &id, &total)
            .unwrap_err()
            .to_string()
            .contains("max_bytes")
    );
    assert!(w.h.join(".npm/_cacache").exists());
}

#[test]
fn rejected_and_executed_proposals_cannot_run_again() {
    let w = World::new();
    let (id, total) = w.propose(&Config::default(), Actor::User, &[".npm/_cacache"], None);
    w.index.reject(&id, "no").unwrap();
    assert!(
        w.execute(&Config::default(), Actor::User, &id, &total)
            .unwrap_err()
            .to_string()
            .contains("rejected")
    );
    let (id, total) = w.propose(&Config::default(), Actor::User, &["code/rs/target"], None);
    w.execute(&Config::default(), Actor::User, &id, &total)
        .unwrap();
    assert!(
        w.execute(&Config::default(), Actor::User, &id, &total)
            .unwrap_err()
            .to_string()
            .contains("executed")
    );
}
