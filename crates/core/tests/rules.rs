mod common;

use std::path::PathBuf;

use common::{home, scan};
use spacehog_core::report::{self, TopQuery};
use spacehog_core::rules::{RuleSet, Safety};

#[test]
fn every_rule_family_claims_its_paths() {
    let (_d, h) = home();
    let state = tempfile::tempdir().unwrap();
    let (index, rules, scan_id) = scan(&h, state.path());
    let expect: &[(&str, &str, &str, Safety)] = &[
        (".npm/_cacache", "npm-cache", "pkg-caches", Safety::Cache),
        (
            "Library/Caches/pip",
            "pip-cache",
            "pkg-caches",
            Safety::Cache,
        ),
        (
            "Library/Caches/Homebrew",
            "homebrew-cache",
            "app-caches",
            Safety::Cache,
        ),
        (
            "Library/Caches/com.apple.Safari",
            "apple-caches",
            "app-caches",
            Safety::Review,
        ),
        (
            "Library/Caches/SomeApp",
            "library-caches",
            "app-caches",
            Safety::Cache,
        ),
        (
            ".cache/huggingface/hub/models--org--llm",
            "hf-models",
            "ml-models",
            Safety::Review,
        ),
        (
            ".cache/random-tool",
            "dot-cache",
            "app-caches",
            Safety::Cache,
        ),
        ("go/pkg/mod", "go-mod-cache", "pkg-caches", Safety::Cache),
        (
            ".cargo/registry",
            "cargo-registry",
            "pkg-caches",
            Safety::Cache,
        ),
        (
            ".rustup/toolchains/stable-aarch64",
            "rustup-toolchains",
            "toolchains",
            Safety::Review,
        ),
        (
            "Library/Developer/Xcode/DerivedData",
            "xcode-derived-data",
            "dev-caches",
            Safety::Regenerable,
        ),
        (
            "Library/Application Support/Code/CachedData",
            "editor-caches",
            "app-caches",
            Safety::Cache,
        ),
        (
            "Library/Application Support/Code",
            "app-support",
            "app-data",
            Safety::Protected,
        ),
        (
            "Library/Application Support/Code/User/settings.json",
            "app-support",
            "app-data",
            Safety::Protected,
        ),
        (
            "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw",
            "docker-desktop-disk",
            "docker-vms",
            Safety::Review,
        ),
        ("Library/Logs", "logs", "logs", Safety::Cache),
        ("Library/Mail", "mail", "user-data", Safety::Protected),
        (".Trash", "trash", "trash", Safety::Cache),
        (
            "Downloads/setup.dmg",
            "installers",
            "downloads",
            Safety::Review,
        ),
        ("Downloads", "downloads", "downloads", Safety::Review),
        (
            "Documents/essay.txt",
            "user-folders",
            "user-data",
            Safety::Protected,
        ),
        (
            "Documents/side/node_modules",
            "node-modules",
            "dev-caches",
            Safety::Regenerable,
        ),
        ("Movies/clip.mp4", "video", "media", Safety::Protected),
        ("loose.png", "images", "media", Safety::Protected),
        ("backup.tar.gz", "archives", "archives", Safety::Review),
        (
            "Applications/Tool.app",
            "applications",
            "apps",
            Safety::Protected,
        ),
        ("code/web", "project", "projects", Safety::Protected),
        (
            "code/web/node_modules",
            "node-modules",
            "dev-caches",
            Safety::Regenerable,
        ),
        (
            "code/web/.next",
            "js-framework-build",
            "dev-builds",
            Safety::Regenerable,
        ),
        (
            "code/web/dist",
            "js-dist",
            "dev-builds",
            Safety::Regenerable,
        ),
        ("code/web/.git", "git", "vcs", Safety::Protected),
        ("code/web/.git/lfs", "git-lfs", "vcs", Safety::Review),
        (
            "code/rs/target",
            "cargo-target",
            "dev-builds",
            Safety::Regenerable,
        ),
        ("code/py/.venv", "py-venv", "dev-envs", Safety::Regenerable),
        (
            "code/py/pkg/__pycache__",
            "py-caches",
            "dev-caches",
            Safety::Regenerable,
        ),
        (
            "code/ios/Pods",
            "cocoapods",
            "dev-caches",
            Safety::Regenerable,
        ),
        (
            "code/tf/.terraform",
            "terraform-dir",
            "dev-caches",
            Safety::Regenerable,
        ),
    ];
    let mut failures = Vec::new();
    for (rel, rule, bucket, safety) in expect {
        let n = index
            .find(scan_id, &h.join(rel))
            .unwrap()
            .unwrap_or_else(|| panic!("{rel} indexed"));
        let item = report::Item::from_node(&rules, &n);
        let got = (
            item.rule_id.as_deref().unwrap_or("-"),
            item.bucket.as_str(),
            item.safety,
        );
        if got != (*rule, *bucket, *safety) {
            failures.push(format!(
                "{rel}: want ({rule}, {bucket}, {safety:?}) got {got:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "misclassified:\n{}",
        failures.join("\n")
    );

    let web_nm = index
        .find(scan_id, &h.join("code/web/node_modules"))
        .unwrap()
        .unwrap();
    assert_eq!(
        web_nm.regen_hint.as_deref(),
        Some("pnpm i"),
        "lockfile picks the package manager"
    );
    assert_eq!(
        web_nm.project_root.as_deref(),
        Some(h.join("code/web").to_str().unwrap())
    );
    let venv = index
        .find(scan_id, &h.join("code/py/.venv"))
        .unwrap()
        .unwrap();
    assert_eq!(venv.regen_hint.as_deref(), Some("uv sync"));
}

#[test]
fn staleness_follows_the_project_not_the_artefact() {
    let (_d, h) = home();
    let state = tempfile::tempdir().unwrap();
    let (index, rules, scan_id) = scan(&h, state.path());
    let old = index
        .find(scan_id, &h.join("code/old/node_modules"))
        .unwrap()
        .unwrap();
    assert!(
        old.stale_days.unwrap() >= 199,
        "untouched project is stale: {:?}",
        old.stale_days
    );
    let web = index
        .find(scan_id, &h.join("code/web/node_modules"))
        .unwrap()
        .unwrap();
    assert!(web.stale_days.unwrap() <= 1, "fresh project is active");

    let stale = report::top(
        &index,
        &rules,
        scan_id,
        &TopQuery {
            category: Some("dev-caches".into()),
            stale_days: Some(90),
            limit: 50,
            ..Default::default()
        },
    )
    .unwrap();
    let paths: Vec<_> = stale.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![h.join("code/old/node_modules").to_str().unwrap()]
    );
}

#[test]
fn summary_accounts_for_every_byte_once() {
    let (_d, h) = home();
    let state = tempfile::tempdir().unwrap();
    let (index, rules, scan_id) = scan(&h, state.path());
    let s = report::summary(&index, &rules, scan_id, 90).unwrap();
    let sum: u64 = s.buckets.iter().map(|b| b.bytes).sum();
    assert_eq!(sum, s.total, "nested claims are not double counted");

    let mut snapshot: Vec<String> = s
        .rules
        .iter()
        .map(|r| format!("{} {} {}", r.bucket, r.rule_id, r.items))
        .collect();
    snapshot.sort();
    let expected = "\
app-caches apple-caches 1
app-caches cache-roots 2
app-caches dot-cache 1
app-caches editor-caches 1
app-caches homebrew-cache 1
app-caches library-caches 1
app-data app-support 2
apps applications 1
archives archives 1
dev-builds cargo-target 1
dev-builds js-dist 1
dev-builds js-framework-build 1
dev-caches cocoapods 1
dev-caches node-modules 3
dev-caches py-caches 1
dev-caches terraform-dir 1
dev-caches xcode-derived-data 1
dev-envs py-venv 1
docker-vms docker-desktop 1
docker-vms docker-desktop-disk 1
downloads downloads 1
downloads installers 1
logs logs 1
media images 1
media video 1
ml-models hf-models 1
ml-models ml-caches 1
pkg-caches cargo-registry 1
pkg-caches go-mod-cache 1
pkg-caches npm-cache 1
pkg-caches pip-cache 1
projects project 1
toolchains rustup-toolchains 1
trash trash 1
user-data mail 1
user-data user-folders 2
vcs git 1
vcs git-lfs 1";
    assert_eq!(snapshot.join("\n"), expected);
}

#[test]
fn user_rules_override_safety_and_paths() {
    let home = PathBuf::from("/home/u");
    let user = r#"
protect = ["~/code/precious"]
[overrides]
node-modules = "review"
"#;
    let rules = RuleSet::from_sources(&[user, spacehog_core::rules::DEFAULT_RULES], &home).unwrap();
    assert_eq!(
        rules.effective_safety(
            "/home/u/x/node_modules",
            Some("node-modules"),
            Some(Safety::Regenerable)
        ),
        Safety::Review
    );
    assert_eq!(
        rules.effective_safety(
            "/home/u/code/precious/node_modules",
            Some("node-modules"),
            Some(Safety::Regenerable)
        ),
        Safety::Protected
    );
}

#[test]
fn the_documented_example_rules_load_and_apply() {
    let home = PathBuf::from("/home/u");
    let example = include_str!("../../../examples/rules.toml");
    let rules =
        RuleSet::from_sources(&[example, spacehog_core::rules::DEFAULT_RULES], &home).unwrap();
    let unity = rules.get("unity-library").expect("example rule is loaded");
    assert_eq!(unity.bucket.as_ref(), "dev-caches");
    assert_eq!(
        rules.effective_safety(
            "/home/u/work/client-deliverables/x",
            Some("js-dist"),
            Some(Safety::Review)
        ),
        Safety::Protected
    );
    assert_eq!(
        rules.effective_safety("/home/u/app/dist", Some("js-dist"), Some(Safety::Review)),
        Safety::Regenerable
    );
    assert_eq!(
        rules.effective_safety("/home/u/scratch/a", None, None),
        Safety::Cache
    );
}
