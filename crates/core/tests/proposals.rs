mod common;

use std::path::PathBuf;

use common::{age, home, scan};
use spacehog_core::config::{Config, DeleteMode};
use spacehog_core::proposals::{Actor, ProposalStatus, ProposeRequest};

fn req(paths: Vec<PathBuf>) -> ProposeRequest {
    ProposeRequest {
        paths,
        rationale: "free space".into(),
        agent: Some("test-agent".into()),
        mode: None,
    }
}

#[test]
fn agents_get_regenerable_and_cache_but_never_protected() {
    let (_d, h) = home();
    let state = tempfile::tempdir().unwrap();
    let (index, rules, _) = scan(&h, state.path());
    let config = Config::default();
    let r = index
        .propose(
            &rules,
            &config,
            Actor::Agent,
            &req(vec![
                h.join("code/web/node_modules"),
                h.join(".npm/_cacache"),
                h.join("code/web/.git"),
                h.join("Documents/essay.txt"),
                h.join("code/web"),
                h.clone(),
                h.join("nope"),
            ]),
        )
        .unwrap();
    let accepted: Vec<_> = r.accepted.iter().map(|i| i.path.clone()).collect();
    assert_eq!(
        accepted.len(),
        2,
        "accepted {accepted:?}, rejected {:?}",
        r.rejected
    );
    let reasons: Vec<_> = r
        .rejected
        .iter()
        .map(|x| {
            (
                x.path.rsplit('/').next().unwrap().to_string(),
                x.reason.clone(),
            )
        })
        .collect();
    assert!(
        reasons
            .iter()
            .any(|(p, r)| p == ".git" && r.starts_with("protected")),
        "{reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|(p, r)| p == "essay.txt" && r.starts_with("protected"))
    );
    assert!(
        reasons
            .iter()
            .any(|(p, r)| p == "web" && r.contains(".git")
                || p == "web" && r.starts_with("protected"))
    );
    assert!(
        reasons
            .iter()
            .any(|(_, r)| r.contains("home directory") || r.contains("scan root"))
    );
    assert!(
        reasons
            .iter()
            .any(|(p, r)| p == "nope" && r.contains("not in the index"))
    );
    assert!(r.requires_approval, "nothing is auto-approved by default");

    let p = index
        .proposal(r.proposal_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(p.status, ProposalStatus::Pending);
    assert_eq!(p.items.len(), 2);
    assert_eq!(p.total_human, r.total_human);
    assert_eq!(p.agent, "test-agent");
}

#[test]
fn agents_need_stale_review_items_users_do_not() {
    let (_d, h) = home();
    age(&h.join("Downloads/setup.dmg"), 10);
    let state = tempfile::tempdir().unwrap();
    let (index, rules, _) = scan(&h, state.path());
    let config = Config::default();
    let dmg = h.join("Downloads/setup.dmg");
    let agent = index
        .propose(&rules, &config, Actor::Agent, &req(vec![dmg.clone()]))
        .unwrap();
    assert!(agent.accepted.is_empty());
    assert!(
        agent.rejected[0]
            .reason
            .contains("review item idle 10 days"),
        "{:?}",
        agent.rejected
    );
    let user = index
        .propose(&rules, &config, Actor::User, &req(vec![dmg]))
        .unwrap();
    assert_eq!(user.accepted.len(), 1);
}

#[test]
fn nested_paths_collapse_and_caps_apply() {
    let (_d, h) = home();
    let state = tempfile::tempdir().unwrap();
    let (index, rules, _) = scan(&h, state.path());
    let paths = vec![
        h.join("code/web/node_modules"),
        h.join("code/web/node_modules/react"),
        h.join(".npm/_cacache"),
    ];
    let config = Config::default();
    let r = index
        .propose(&rules, &config, Actor::Agent, &req(paths.clone()))
        .unwrap();
    assert_eq!(r.accepted.len(), 2);
    assert_eq!(r.rejected.len(), 1);
    assert!(
        r.rejected[0].reason.starts_with("already covered by"),
        "{:?}",
        r.rejected
    );

    let mut capped = Config::default();
    capped.cleanup.max_items = 1;
    let r = index
        .propose(&rules, &capped, Actor::Agent, &req(paths))
        .unwrap();
    assert_eq!(r.accepted.len(), 1);
    assert!(
        r.rejected
            .iter()
            .all(|x| x.reason.starts_with("exceeds max_items")),
        "{:?}",
        r.rejected
    );

    let mut tiny = Config::default();
    tiny.cleanup.max_bytes_per_execution = 1;
    let r = index
        .propose(
            &rules,
            &tiny,
            Actor::Agent,
            &req(vec![h.join(".npm/_cacache")]),
        )
        .unwrap();
    assert!(r.accepted.is_empty() && r.proposal_id.is_none());
    assert!(
        r.rejected[0]
            .reason
            .starts_with("exceeds max_bytes_per_execution")
    );
}

#[test]
fn auto_approve_never_covers_permanent() {
    let (_d, h) = home();
    let state = tempfile::tempdir().unwrap();
    let (index, rules, _) = scan(&h, state.path());
    let mut config = Config::default();
    config.mcp.auto_approve = vec![
        spacehog_core::rules::Safety::Regenerable,
        spacehog_core::rules::Safety::Cache,
    ];
    let trash = index
        .propose(
            &rules,
            &config,
            Actor::Agent,
            &req(vec![h.join(".npm/_cacache")]),
        )
        .unwrap();
    assert!(!trash.requires_approval);
    let mut perm = req(vec![h.join("code/rs/target")]);
    perm.mode = Some(DeleteMode::Permanent);
    let perm = index.propose(&rules, &config, Actor::Agent, &perm).unwrap();
    assert!(perm.requires_approval);

    let id = trash.proposal_id.unwrap();
    index.approve(&id).unwrap();
    assert!(index.approve(&id).is_err(), "approving twice is refused");
    index.reject(&id, "changed my mind").unwrap();
    assert_eq!(
        index.proposal(&id).unwrap().unwrap().reason.as_deref(),
        Some("changed my mind")
    );
}
