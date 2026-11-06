use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::Labeler;
use crate::model::{Label, NodeKind};
use crate::rules::{Rule, RuleSet, Safety, StaleBasis};
use crate::scan::{FlatNode, ScanTree};

/// Files whose mtime marks activity in a project directory.
const PROJECT_MARKERS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "Cargo.toml",
    "Cargo.lock",
    "go.mod",
    "go.sum",
    "pyproject.toml",
    "requirements.txt",
    "uv.lock",
    "poetry.lock",
    "setup.py",
    "Podfile",
    "Podfile.lock",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "pom.xml",
    "mix.exs",
    "pubspec.yaml",
    "Gemfile",
    "Makefile",
    "CMakeLists.txt",
];

/// How far up from an artefact to look for the owning project root.
const PROJECT_SEARCH_DEPTH: usize = 6;

pub struct Categoriser {
    pub rules: Arc<RuleSet>,
}

impl Categoriser {
    pub fn new(rules: Arc<RuleSet>) -> Self {
        Self { rules }
    }
}

impl Labeler for Categoriser {
    fn label(&self, tree: &ScanTree) -> Vec<Label> {
        label_tree(&self.rules, tree)
    }
}

struct Ctx<'a> {
    tree: &'a ScanTree,
}

impl Ctx<'_> {
    fn node(&self, i: u32) -> &FlatNode {
        &self.tree.nodes[i as usize]
    }

    /// Direct children of `i`, skipping over each child's subtree.
    fn children(&self, i: u32) -> impl Iterator<Item = u32> + '_ {
        let end = self.node(i).last_desc;
        let mut j = i + 1;
        std::iter::from_fn(move || {
            if j > end {
                return None;
            }
            let cur = j;
            j = self.node(cur).last_desc + 1;
            Some(cur)
        })
    }

    fn child_named(&self, i: u32, name: &str) -> Option<u32> {
        self.children(i).find(|&c| &*self.node(c).name == name)
    }

    fn any_child_matches(&self, i: u32, set: &globset::GlobSet) -> bool {
        self.children(i).any(|c| set.is_match(&*self.node(c).name))
    }

    fn rule_holds(&self, rule: &Rule, i: u32) -> bool {
        let n = self.node(i);
        if let Some(sib) = &rule.sibling {
            let Some(p) = n.parent else { return false };
            if !self
                .children(p)
                .any(|c| c != i && sib.is_match(&*self.node(c).name))
            {
                return false;
            }
        }
        if let Some(contains) = &rule.contains
            && !self.any_child_matches(i, contains)
        {
            return false;
        }
        true
    }

    fn safety(&self, rule: &Rule, i: u32, path: &Path) -> Safety {
        let Some(promote) = rule.promote() else {
            return rule.safety();
        };
        let Some(parent) = self.node(i).parent else {
            return rule.safety();
        };
        let lock_ok = rule
            .promote_sibling()
            .is_none_or(|set| self.any_child_matches(parent, set));
        let script_ok = promote.package_script.as_deref().is_none_or(|script| {
            let manifest = path.parent().map(|p| p.join("package.json"));
            manifest
                .and_then(|m| std::fs::read(m).ok())
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .is_some_and(|v| v.get("scripts").and_then(|s| s.get(script)).is_some())
        });
        if lock_ok && script_ok {
            promote.safety
        } else {
            rule.safety()
        }
    }

    fn regen(&self, rule: &Rule, i: u32) -> Option<Arc<str>> {
        if let Some(p) = self.node(i).parent {
            for (set, cmd) in &rule.regen_cmd {
                if self.any_child_matches(p, set) {
                    return Some(cmd.as_str().into());
                }
            }
        }
        rule.def.regen.as_deref().map(Into::into)
    }

    /// Last activity in the project that owns an artefact, and that project's root.
    fn project_activity(&self, i: u32, path: &Path) -> (Option<i64>, Option<PathBuf>) {
        let mut newest: Option<i64> = None;
        let mut bump = |t: i64| newest = Some(newest.map_or(t, |n: i64| n.max(t)));
        let mut root = None;
        let mut cur = self.node(i).parent;
        let mut dir = path.parent().map(Path::to_path_buf);
        for depth in 0..PROJECT_SEARCH_DEPTH {
            let (Some(p), Some(d)) = (cur, dir.clone()) else {
                break;
            };
            let pn = self.node(p);
            if depth == 0 {
                bump(pn.mtime);
                for c in self.children(p) {
                    let cn = self.node(c);
                    if c != i && cn.kind == NodeKind::File {
                        bump(cn.mtime);
                    }
                }
            }
            let mut is_root = false;
            for c in self.children(p) {
                let cn = self.node(c);
                if PROJECT_MARKERS.contains(&&*cn.name) {
                    bump(cn.mtime);
                    is_root = true;
                } else if &*cn.name == ".git" {
                    is_root = true;
                    for marker in ["HEAD", "index", "FETCH_HEAD"] {
                        if let Some(m) = self.child_named(c, marker) {
                            bump(self.node(m).mtime);
                        }
                    }
                }
            }
            let next = d.parent().map(Path::to_path_buf);
            if is_root {
                root = Some(d);
                if self.child_named(p, ".git").is_some() {
                    break;
                }
            }
            cur = pn.parent;
            dir = next;
        }
        (
            newest,
            root.or_else(|| path.parent().map(Path::to_path_buf)),
        )
    }
}

fn ext_of(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then_some(ext)
}

struct Claim {
    last_desc: u32,
    label: Label,
}

pub fn label_tree(rules: &RuleSet, tree: &ScanTree) -> Vec<Label> {
    let ctx = Ctx { tree };
    let n = tree.nodes.len();
    let mut labels = vec![Label::default(); n];
    let mut dirs: Vec<(u32, PathBuf)> = Vec::new();
    let mut fallbacks: Vec<Claim> = Vec::new();
    let mut i = 0u32;
    while (i as usize) < n {
        while dirs.last().is_some_and(|(d, _)| ctx.node(*d).last_desc < i) {
            dirs.pop();
        }
        while fallbacks.last().is_some_and(|c| c.last_desc < i) {
            fallbacks.pop();
        }
        let node = ctx.node(i);
        let is_dir = node.kind == NodeKind::Dir;
        let parent_path = dirs.last().map(|(_, p)| p.as_path());
        let path = match parent_path {
            Some(p) => p.join(&*node.name),
            None => PathBuf::from(&*node.name),
        };
        let hits = {
            let path_str = path.to_string_lossy();
            let need_path = is_dir || rules.file_needs_path(&path_str);
            rules.hits(
                &node.name,
                need_path.then_some(&*path_str),
                ext_of(&node.name),
            )
        };
        let matched = rules
            .candidates(&hits, is_dir)
            .into_iter()
            .map(|r| &rules.rules[r])
            .find(|r| ctx.rule_holds(r, i));

        if let Some(rule) = matched {
            let (stale_ref, project_root) = match rule.def.stale {
                StaleBasis::Project => ctx.project_activity(i, &path),
                StaleBasis::SelfMtime => (Some(node.max_mtime), None),
                StaleBasis::None => (None, None),
            };
            let label = Label {
                rule_id: Some(rule.id.clone()),
                bucket: Some(rule.bucket.clone()),
                claim: true,
                stale_ref,
                project_root: project_root.map(|p| p.to_string_lossy().into()),
                excl_disk: node.disk,
                excl_apparent: node.apparent,
                regen_hint: ctx.regen(rule, i),
                safety: Some(ctx.safety(rule, i, &path)),
            };
            let inherited = Label {
                rule_id: label.rule_id.clone(),
                bucket: label.bucket.clone(),
                safety: label.safety,
                ..Label::default()
            };
            if rule.def.fallback && is_dir {
                labels[i as usize] = label.clone();
                fallbacks.push(Claim {
                    last_desc: node.last_desc,
                    label: inherited,
                });
                dirs.push((i, path));
                i += 1;
            } else {
                labels[i as usize] = label;
                for j in i + 1..=node.last_desc {
                    labels[j as usize] = inherited.clone();
                }
                i = node.last_desc + 1;
            }
            continue;
        }

        if let Some(ctx_claim) = fallbacks.last() {
            labels[i as usize] = ctx_claim.label.clone();
        } else if !is_dir {
            labels[i as usize].bucket = Some("other".into());
        }
        if is_dir {
            dirs.push((i, path));
        }
        i += 1;
    }
    subtract_nested_claims(tree, &mut labels);
    labels
}

/// Makes each claim's `excl_*` cover only bytes no nested claim accounts for,
/// so summing claims never double counts a fallback and what it contains.
fn subtract_nested_claims(tree: &ScanTree, labels: &mut [Label]) {
    let mut open: Vec<usize> = Vec::new();
    for i in 0..tree.nodes.len() {
        if !labels[i].claim {
            continue;
        }
        while open
            .last()
            .is_some_and(|&c| (tree.nodes[c].last_desc as usize) < i)
        {
            open.pop();
        }
        if let Some(&outer) = open.last() {
            let (d, a) = (tree.nodes[i].disk, tree.nodes[i].apparent);
            labels[outer].excl_disk = labels[outer].excl_disk.saturating_sub(d);
            labels[outer].excl_apparent = labels[outer].excl_apparent.saturating_sub(a);
        }
        open.push(i);
    }
}
