//! Tool arguments. Field doc comments become the schema descriptions agents
//! read. Enum fields deserialise straight into the core types so core stays
//! the one definition of their values; the `*Schema` mirrors only describe
//! those values to clients.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use spacehog_core::config::DeleteMode;
use spacehog_core::proposals::ProposalStatus;
use spacehog_core::query::{KindFilter, Sort};
use spacehog_core::rules::Safety;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanArgs {
    /// Directory to scan. Defaults to the configured root, usually your home directory.
    pub root: Option<PathBuf>,
    /// Block until the scan finishes and return its summary. A home-directory scan can
    /// take minutes, so prefer leaving this off and polling `status`.
    #[serde(default)]
    pub wait: bool,
    /// Rescan only what changed. Not supported yet (planned for v1.5); a full scan runs instead.
    #[serde(default)]
    pub incremental: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryArgs {
    /// Scan to summarise. Defaults to the latest finished scan.
    pub scan_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopArgs {
    /// Maximum items to return. Defaults to 20.
    pub limit: Option<usize>,
    /// A bucket id (e.g. `dev-caches`), a category, or a rule id (e.g. `node-modules`).
    pub category: Option<String>,
    /// Only items at least this many bytes.
    pub min_size: Option<u64>,
    /// Only items idle at least this many days.
    pub stale_days: Option<i64>,
    /// Only items in these safety classes.
    #[schemars(with = "Option<Vec<SafetySchema>>")]
    pub safety: Option<Vec<Safety>>,
    /// Only items under this absolute path.
    pub under: Option<PathBuf>,
    /// Only directories, only files, or both. Defaults to `any`.
    #[schemars(with = "Option<KindSchema>")]
    pub kind: Option<KindFilter>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryArgs {
    /// Glob matched against the node's name, not its path (e.g. `*.dmg`).
    pub glob: Option<String>,
    /// Only nodes under this absolute path.
    pub under: Option<PathBuf>,
    /// Only nodes at least this many bytes on disk.
    pub min_size: Option<u64>,
    /// Depth limit, relative to `under` when given, else to the scan root.
    pub max_depth: Option<u32>,
    /// `size` (largest first, the default) or `mtime` (oldest first).
    #[schemars(with = "Option<SortSchema>")]
    pub sort: Option<Sort>,
    /// Maximum nodes to return. Defaults to 20.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplainArgs {
    /// Absolute path to explain.
    pub path: PathBuf,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposeArgs {
    /// Absolute paths to clean up.
    pub paths: Vec<PathBuf>,
    /// Why these items are safe to remove, shown to the human who approves the proposal.
    pub rationale: String,
    /// Your name as the human should see it. Defaults to the MCP client's name.
    pub agent: Option<String>,
    /// `trash` (the default, recoverable) or `permanent`. Permanent always needs approval.
    #[schemars(with = "Option<ModeSchema>")]
    pub mode: Option<DeleteMode>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListProposalsArgs {
    /// Only proposals in this state. Defaults to every state.
    #[schemars(with = "Option<StatusSchema>")]
    pub status: Option<ProposalStatus>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FreeSpaceArgs {
    /// How many gigabytes to free, e.g. `20`.
    pub gb: String,
}

/// Declares a schema mirror of a core enum. The `From` match is exhaustive over
/// the core type, so a new core variant fails to compile until it is mirrored.
macro_rules! mirror {
    ($mirror:ident for $core:ident { $($v:ident),+ }) => {
        #[derive(Serialize, JsonSchema)]
        #[serde(rename_all = "lowercase")]
        pub enum $mirror { $($v),+ }

        impl From<$core> for $mirror {
            fn from(v: $core) -> Self {
                match v { $($core::$v => Self::$v),+ }
            }
        }
    };
}

mirror!(SafetySchema for Safety { Regenerable, Cache, Review, Protected });
mirror!(KindSchema for KindFilter { Dir, File, Any });
mirror!(SortSchema for Sort { Size, Mtime });
mirror!(ModeSchema for DeleteMode { Trash, Permanent });
mirror!(StatusSchema for ProposalStatus { Pending, Approved, Rejected, Executed, Failed });

#[cfg(test)]
mod tests {
    use super::*;

    fn values<T: JsonSchema>() -> Vec<String> {
        let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
        schema["enum"]
            .as_array()
            .or_else(|| schema["oneOf"].as_array())
            .expect("an enum schema")
            .iter()
            .map(|v| {
                v.as_str()
                    .or_else(|| v["const"].as_str())
                    .or_else(|| v["enum"][0].as_str())
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    fn parses_as<T: serde::de::DeserializeOwned>(advertised: &[String]) {
        for v in advertised {
            serde_json::from_value::<T>(serde_json::json!(v))
                .unwrap_or_else(|e| panic!("schema advertises {v:?}, core rejects it: {e}"));
        }
    }

    #[test]
    fn advertised_enum_values_parse_as_core_types() {
        let safety = values::<SafetySchema>();
        assert_eq!(safety, ["regenerable", "cache", "review", "protected"]);
        parses_as::<Safety>(&safety);
        parses_as::<KindFilter>(&values::<KindSchema>());
        parses_as::<Sort>(&values::<SortSchema>());
        parses_as::<DeleteMode>(&values::<ModeSchema>());
        parses_as::<ProposalStatus>(&values::<StatusSchema>());
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteArgs {
    /// The id `propose_cleanup` returned.
    pub proposal_id: String,
    /// Must equal the proposal's `total_human` exactly, e.g. "28.4 GB". This
    /// proves you read the proposal you are executing.
    pub confirm: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoArgs {
    /// The `journal_id` an `execute_cleanup` result returned.
    pub journal_id: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangesArgs {
    /// A duration such as "7d", "2w", or "1m", or a scan id as a string. Defaults to the
    /// previous scan of the same root.
    pub since: Option<String>,
    /// Paths to return per direction (grown and shrunk). Defaults to 30.
    pub limit: Option<usize>,
}
