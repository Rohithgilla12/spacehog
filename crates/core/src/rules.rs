use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const DEFAULT_RULES: &str = include_str!("../rules/default.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    Regenerable,
    Cache,
    Review,
    Protected,
}

impl Safety {
    pub fn as_str(self) -> &'static str {
        match self {
            Safety::Regenerable => "regenerable",
            Safety::Cache => "cache",
            Safety::Review => "review",
            Safety::Protected => "protected",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "regenerable" => Some(Safety::Regenerable),
            "cache" => Some(Safety::Cache),
            "review" => Some(Safety::Review),
            "protected" => Some(Safety::Protected),
            _ => None,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Safety::Regenerable => "♻",
            Safety::Cache => "◌",
            Safety::Review => "◐",
            Safety::Protected => "⛨",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StaleBasis {
    Project,
    #[serde(rename = "self")]
    SelfMtime,
    #[default]
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KindMatch {
    Dir,
    File,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketDef {
    pub id: String,
    pub label: String,
    pub colour: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegenCmd {
    pub sibling: String,
    pub cmd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Promote {
    pub safety: Safety,
    #[serde(default)]
    pub sibling: Vec<String>,
    pub package_script: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    pub id: String,
    pub category: String,
    pub label: String,
    #[serde(default)]
    pub name: Vec<String>,
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(default)]
    pub ext: Vec<String>,
    pub kind: Option<KindMatch>,
    #[serde(default)]
    pub sibling: Vec<String>,
    #[serde(default)]
    pub contains: Vec<String>,
    pub safety: Safety,
    pub regen: Option<String>,
    #[serde(default)]
    pub regen_cmd: Vec<RegenCmd>,
    #[serde(default)]
    pub stale: StaleBasis,
    #[serde(default)]
    pub fallback: bool,
    pub promote: Option<Promote>,
    pub icon: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    #[serde(default)]
    bucket: Vec<BucketDef>,
    #[serde(default)]
    categories: HashMap<String, String>,
    #[serde(default)]
    rule: Vec<RuleDef>,
    #[serde(default)]
    overrides: HashMap<String, Safety>,
    #[serde(default)]
    protect: Vec<String>,
    #[serde(default)]
    allow: Vec<String>,
}

/// A compiled rule. `def` keeps the source for `rules()` and `explain`.
#[derive(Debug)]
pub struct Rule {
    pub def: RuleDef,
    pub id: Arc<str>,
    pub bucket: Arc<str>,
    pub kind: KindMatch,
    pub sibling: Option<GlobSet>,
    pub contains: Option<GlobSet>,
    pub regen_cmd: Vec<(GlobSet, String)>,
    promote_sibling: Option<GlobSet>,
    has_name: bool,
    has_path: bool,
    has_ext: bool,
}

impl Rule {
    pub fn safety(&self) -> Safety {
        self.def.safety
    }

    pub fn promote(&self) -> Option<&Promote> {
        self.def.promote.as_ref()
    }

    pub fn promote_sibling(&self) -> Option<&GlobSet> {
        self.promote_sibling.as_ref()
    }
}

#[derive(Debug)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    pub buckets: Vec<BucketDef>,
    by_id: HashMap<String, usize>,
    name_set: GlobSet,
    name_owner: Vec<usize>,
    path_set: GlobSet,
    path_owner: Vec<usize>,
    /// Literal prefixes of path rules that can match files, so most files skip path matching.
    file_path_prefixes: Vec<String>,
    ext_map: HashMap<String, Vec<usize>>,
    overrides: HashMap<String, Safety>,
    protect: GlobSet,
    allow: GlobSet,
    pub home: PathBuf,
}

pub struct Hits {
    names: Vec<usize>,
    paths: Option<Vec<usize>>,
    exts: Vec<usize>,
}

fn path_glob(pattern: &str) -> Result<Glob> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| Error::Rules(format!("{pattern}: {e}")))
}

fn name_glob(pattern: &str) -> Result<Glob> {
    Glob::new(pattern).map_err(|e| Error::Rules(format!("{pattern}: {e}")))
}

fn set_of(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(name_glob(p)?);
    }
    Ok(Some(b.build().map_err(|e| Error::Rules(e.to_string()))?))
}

/// Expands `~` and `$VAR`. Returns None when a referenced variable is unset.
pub fn expand(pattern: &str, home: &Path) -> Option<String> {
    let s = if pattern == "~" {
        home.to_string_lossy().into_owned()
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        format!("{}/{}", home.to_string_lossy().trim_end_matches('/'), rest)
    } else {
        pattern.to_string()
    };
    if let Some(var) = s.strip_prefix('$') {
        let (name, rest) = var.split_once('/').map_or((var, ""), |(n, r)| (n, r));
        let val = std::env::var(name).ok().filter(|v| !v.is_empty())?;
        return Some(if rest.is_empty() {
            val
        } else {
            format!("{val}/{rest}")
        });
    }
    Some(s)
}

fn literal_prefix(pattern: &str) -> String {
    let end = pattern.find(['*', '?', '[', '{']).unwrap_or(pattern.len());
    pattern[..end].to_string()
}

impl RuleSet {
    pub fn default_rules(home: &Path) -> Result<Self> {
        Self::from_sources(&[DEFAULT_RULES], home)
    }

    /// Loads the embedded defaults plus an optional user file. User rules are
    /// evaluated before the defaults; user buckets and categories extend them.
    pub fn load(user_rules: Option<&Path>, home: &Path) -> Result<Self> {
        match user_rules {
            Some(p) if p.exists() => {
                let user = std::fs::read_to_string(p)?;
                Self::from_sources(&[&user, DEFAULT_RULES], home)
            }
            _ => Self::default_rules(home),
        }
    }

    pub fn from_sources(sources: &[&str], home: &Path) -> Result<Self> {
        let mut merged = RuleFile::default();
        for src in sources {
            let f: RuleFile = toml::from_str(src).map_err(|e| Error::Rules(e.to_string()))?;
            for b in f.bucket {
                if !merged.bucket.iter().any(|x| x.id == b.id) {
                    merged.bucket.push(b);
                }
            }
            for (k, v) in f.categories {
                merged.categories.entry(k).or_insert(v);
            }
            merged.rule.extend(f.rule);
            for (k, v) in f.overrides {
                merged.overrides.entry(k).or_insert(v);
            }
            merged.protect.extend(f.protect);
            merged.allow.extend(f.allow);
        }
        Self::compile(merged, home)
    }

    fn compile(file: RuleFile, home: &Path) -> Result<Self> {
        let mut name_b = GlobSetBuilder::new();
        let mut name_owner = Vec::new();
        let mut path_b = GlobSetBuilder::new();
        let mut path_owner = Vec::new();
        let mut ext_map: HashMap<String, Vec<usize>> = HashMap::new();
        let mut file_path_prefixes = Vec::new();
        let mut rules = Vec::new();
        let mut by_id = HashMap::new();
        for def in file.rule {
            if by_id.contains_key(&def.id) {
                continue;
            }
            let idx = rules.len();
            let bucket = file.categories.get(&def.category).ok_or_else(|| {
                Error::Rules(format!(
                    "rule {}: unknown category {}",
                    def.id, def.category
                ))
            })?;
            if !file.bucket.iter().any(|b| &b.id == bucket) {
                return Err(Error::Rules(format!(
                    "category {} maps to unknown bucket {bucket}",
                    def.category
                )));
            }
            if def.name.is_empty() && def.path.is_empty() && def.ext.is_empty() {
                return Err(Error::Rules(format!(
                    "rule {} has no name, path, or ext matcher",
                    def.id
                )));
            }
            let kind = def.kind.unwrap_or(if def.ext.is_empty() {
                KindMatch::Dir
            } else {
                KindMatch::File
            });
            for n in &def.name {
                name_b.add(name_glob(n)?);
                name_owner.push(idx);
            }
            for p in &def.path {
                let Some(expanded) = expand(p, home) else {
                    continue;
                };
                path_b.add(path_glob(&expanded)?);
                path_owner.push(idx);
                if kind != KindMatch::Dir {
                    file_path_prefixes.push(literal_prefix(&expanded));
                }
            }
            for e in &def.ext {
                ext_map.entry(e.to_ascii_lowercase()).or_default().push(idx);
            }
            let regen_cmd = def
                .regen_cmd
                .iter()
                .map(|r| {
                    Ok((
                        set_of(std::slice::from_ref(&r.sibling))?.expect("one pattern"),
                        r.cmd.clone(),
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            by_id.insert(def.id.clone(), idx);
            rules.push(Rule {
                id: def.id.as_str().into(),
                bucket: bucket.as_str().into(),
                kind,
                sibling: set_of(&def.sibling)?,
                contains: set_of(&def.contains)?,
                regen_cmd,
                promote_sibling: def
                    .promote
                    .as_ref()
                    .map(|p| set_of(&p.sibling))
                    .transpose()?
                    .flatten(),
                has_name: !def.name.is_empty(),
                has_path: !def.path.is_empty(),
                has_ext: !def.ext.is_empty(),
                def,
            });
        }
        let globs = |pats: &[String]| -> Result<GlobSet> {
            let mut b = GlobSetBuilder::new();
            for p in pats {
                if let Some(e) = expand(p, home) {
                    b.add(path_glob(&e)?);
                    b.add(path_glob(&format!("{}/**", e.trim_end_matches('/')))?);
                }
            }
            b.build().map_err(|e| Error::Rules(e.to_string()))
        };
        Ok(Self {
            by_id,
            name_set: name_b.build().map_err(|e| Error::Rules(e.to_string()))?,
            name_owner,
            path_set: path_b.build().map_err(|e| Error::Rules(e.to_string()))?,
            path_owner,
            file_path_prefixes,
            ext_map,
            overrides: file.overrides,
            protect: globs(&file.protect)?,
            allow: globs(&file.allow)?,
            buckets: file.bucket,
            rules,
            home: home.to_path_buf(),
        })
    }

    pub fn get(&self, id: &str) -> Option<&Rule> {
        self.by_id.get(id).map(|&i| &self.rules[i])
    }

    pub fn bucket(&self, id: &str) -> Option<&BucketDef> {
        self.buckets.iter().find(|b| b.id == id)
    }

    pub fn bucket_index(&self, id: &str) -> usize {
        self.buckets
            .iter()
            .position(|b| b.id == id)
            .unwrap_or(self.buckets.len())
    }

    /// Rule ids whose bucket or category or id equals `key`.
    pub fn resolve_category(&self, key: &str) -> Option<Vec<String>> {
        let ids: Vec<String> = self
            .rules
            .iter()
            .filter(|r| &*r.bucket == key || r.def.category == key || &*r.id == key)
            .map(|r| r.id.to_string())
            .collect();
        (!ids.is_empty()).then_some(ids)
    }

    /// The safety that applies to a path whose index label says `recorded`.
    /// Path-level `protect` wins, then `allow`, then per-rule overrides.
    pub fn effective_safety(
        &self,
        path: &str,
        rule_id: Option<&str>,
        recorded: Option<Safety>,
    ) -> Safety {
        if self.protect.is_match(path) {
            return Safety::Protected;
        }
        if self.allow.is_match(path) {
            return Safety::Cache;
        }
        if let Some(s) = rule_id.and_then(|id| self.overrides.get(id)) {
            return *s;
        }
        recorded.unwrap_or(Safety::Protected)
    }

    pub fn file_needs_path(&self, path_prefix_check: &str) -> bool {
        self.file_path_prefixes
            .iter()
            .any(|p| path_prefix_check.starts_with(p.as_str()))
    }

    pub fn hits(&self, name: &str, path: Option<&str>, ext: Option<&str>) -> Hits {
        Hits {
            names: self
                .name_set
                .matches(name)
                .into_iter()
                .map(|i| self.name_owner[i])
                .collect(),
            paths: path.map(|p| {
                self.path_set
                    .matches(p)
                    .into_iter()
                    .map(|i| self.path_owner[i])
                    .collect()
            }),
            exts: ext
                .and_then(|e| self.ext_map.get(&e.to_ascii_lowercase()))
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// Rules whose name, path, and ext matchers all hold, in priority order.
    /// Kind and marker checks are left to the caller, which owns the tree.
    pub fn candidates(&self, hits: &Hits, is_dir: bool) -> Vec<usize> {
        let mut c: Vec<usize> = hits.names.iter().chain(&hits.exts).copied().collect();
        if let Some(p) = &hits.paths {
            c.extend(p);
        }
        c.sort_unstable();
        c.dedup();
        c.retain(|&i| {
            let r = &self.rules[i];
            let kind_ok = match r.kind {
                KindMatch::Dir => is_dir,
                KindMatch::File => !is_dir,
                KindMatch::Any => true,
            };
            kind_ok
                && (!r.has_name || hits.names.contains(&i))
                && (!r.has_ext || hits.exts.contains(&i))
                && (!r.has_path || hits.paths.as_ref().is_some_and(|p| p.contains(&i)))
        });
        c
    }
}
