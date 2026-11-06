mod common;

use std::fs;

use common::{home, scan, write};
use spacehog_core::changes::{ChangeKind, Since, changes};

#[test]
fn growth_is_attributed_to_the_deepest_dominant_path() {
    let (_d, h) = home();
    write(&h.join("code/old/node_modules/big/blob"), 3_000_000);
    let state = tempfile::tempdir().unwrap();
    let (index, rules, first) = scan(&h, state.path());

    write(
        &h.join(".cache/huggingface/hub/models--org--llm/blobs/w2"),
        8_000_000,
    );
    write(&h.join("Downloads/new-installer.dmg"), 2_000_000);
    fs::remove_dir_all(h.join("code/old/node_modules")).unwrap();
    drop(index);
    let (index, rules2, second) = scan(&h, state.path());
    assert_ne!(first, second);
    let _ = rules;

    let c = changes(&index, &rules2, None, Since::Previous, 30).unwrap();
    assert_eq!(c.from.id, first);
    assert_eq!(c.to.id, second);
    let top = &c.grown[0];
    assert_eq!(
        top.path,
        h.join(".cache/huggingface/hub/models--org--llm/blobs")
            .to_string_lossy()
    );
    assert_eq!(top.change, ChangeKind::Grown);
    assert!(top.delta >= 8_000_000);
    assert!(
        c.grown
            .iter()
            .any(|d| d.path.ends_with("new-installer.dmg") && d.change == ChangeKind::New)
    );
    assert!(
        c.grown.iter().all(|d| d.path != h.to_string_lossy()),
        "home itself is never the answer"
    );
    let gone = c
        .shrunk
        .iter()
        .find(|d| d.path.ends_with("code/old/node_modules"))
        .expect("deletion reported");
    assert_eq!(gone.change, ChangeKind::Gone);

    let ml = c.buckets.iter().find(|b| b.id == "ml-models").unwrap();
    assert!(ml.delta >= 8_000_000, "bucket growth: {:?}", c.buckets);
    let dev = c.buckets.iter().find(|b| b.id == "dev-caches").unwrap();
    assert!(dev.delta <= -3_000_000);
}

#[test]
fn a_single_scan_has_nothing_to_compare() {
    let (_d, h) = home();
    let state = tempfile::tempdir().unwrap();
    let (index, rules, _) = scan(&h, state.path());
    let err = changes(&index, &rules, None, Since::Previous, 30).unwrap_err();
    assert!(err.to_string().contains("only one scan"));
}
