use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, ListResourcesResult, PaginatedRequestParams, PromptMessage,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData, Peer, RoleServer, ServerHandler, prompt, prompt_handler, prompt_router, tool,
    tool_handler, tool_router,
};
use serde::Serialize;
use serde_json::json;
use spacehog_core::categorise::Categoriser;
use spacehog_core::config::Config;
use spacehog_core::engine::{ScanJob, ScanRequest, spawn_scan};
use spacehog_core::model::{NodeKind, ScanId, ScanRecord, ScanStatus};
use spacehog_core::proposals::{Actor, ProposalStatus, ProposeRequest};
use spacehog_core::query::NodeFilter;
use spacehog_core::report::{self, Summary, TopQuery};
use spacehog_core::rules::{RuleSet, Safety};
use spacehog_core::{Error as CoreError, Index, Result as CoreResult, human, paths, status};

use crate::args::{
    ExplainArgs, FreeSpaceArgs, ListProposalsArgs, ProposeArgs, QueryArgs, ScanArgs, SummaryArgs,
    TopArgs,
};

const INSTRUCTIONS: &str = "\
spacehog indexes disk usage and classifies large items by safety: `regenerable` (build output a \
command recreates), `cache`, `review` (a human should look first), and `protected` (never cleaned).

Start with `status`. If there is no recent scan, call `scan` and poll `status` until it finishes. \
Use `summary` and `top` to find candidates, preferring regenerable and cache items and stale ones \
first. Call `explain` on every path before you propose it, and always for `review` items.

You cannot delete anything. `propose_cleanup` records a proposal; a human approves it in the \
spacehog TUI or with `spacehog proposals approve <id>`, so tell the user the proposal id and ask \
them to approve it. Execution needs that approval unless the user has listed every item's safety \
class in `mcp.auto_approve`; `requires_approval` in the result says which applies.

Sizes are bytes with a human-readable string beside them. Paths are absolute.";

const SUMMARY_URI: &str = "spacehog://summary";
const PROPOSALS_URI: &str = "spacehog://proposals";

/// Everything a tool call needs. Shared read-only across calls except the
/// scan slot. The index is opened per call because a rusqlite connection is
/// not `Sync`, and opening one is a file open plus a few pragmas.
pub(crate) struct State {
    db: PathBuf,
    lock_dir: PathBuf,
    config: Config,
    rules: Arc<RuleSet>,
    /// The scan this server started, kept so `scan` with `wait` can join it.
    /// The lock file stays the source of truth for whether any scan runs.
    scan: Mutex<Option<ScanJob>>,
}

impl State {
    pub(crate) fn from_env() -> CoreResult<Self> {
        let config = Config::load()?;
        let rules = RuleSet::load(Some(&config.rules_path()), &paths::home_dir())?;
        Ok(Self {
            db: paths::db_path(),
            lock_dir: paths::data_dir(),
            config,
            rules: Arc::new(rules),
            scan: Mutex::new(None),
        })
    }

    fn index(&self) -> CoreResult<Index> {
        Index::open(&self.db)
    }

    fn status(&self) -> CoreResult<StatusView> {
        let s = status::status(&self.index()?)?;
        Ok(StatusView {
            volumes: s.volumes,
            last_scan: s.last_scan.map(ScanView::from),
            scan_in_progress: s.scan_in_progress.map(ScanView::from),
            pending_proposals: s.pending_proposals,
        })
    }

    fn scan(&self, args: ScanArgs) -> CoreResult<ScanReply> {
        let (scan_id, started) = self.start_or_attach(args.root)?;
        let note = args.incremental.then_some(
            "incremental scans are not supported yet (planned for v1.5); this is a full scan",
        );
        if !args.wait {
            return Ok(ScanReply {
                scan_id,
                started,
                status: ScanStatus::Running,
                note,
                summary: None,
            });
        }
        self.wait_for(scan_id)?;
        Ok(ScanReply {
            scan_id,
            started,
            status: ScanStatus::Done,
            note,
            summary: Some(self.summary(Some(scan_id))?),
        })
    }

    /// Starts a scan, or returns the one already running here or in another process.
    fn start_or_attach(&self, root: Option<PathBuf>) -> CoreResult<(ScanId, bool)> {
        let mut slot = self.scan.lock().expect("scan slot poisoned");
        if let Some(job) = slot.take_if(|j| j.is_finished()) {
            reap(job);
        }
        if let Some(job) = slot.as_ref() {
            return Ok((job.scan_id, false));
        }
        let root = root.map_or_else(|| self.config.default_root(), |p| resolve(&p));
        let mut req = ScanRequest::new(root);
        req.db = self.db.clone();
        req.lock_dir = self.lock_dir.clone();
        req.keep = self.config.keep_scans;
        req.options.one_file_system = self.config.one_file_system;
        req.labeler = Some(Arc::new(Categoriser::new(self.rules.clone())));
        match spawn_scan(req) {
            Ok(job) => {
                tracing::info!(scan_id = job.scan_id, root = %job.root.display(), "scan started");
                let id = job.scan_id;
                *slot = Some(job);
                Ok((id, true))
            }
            Err(CoreError::ScanInProgress { scan_id, pid }) => {
                tracing::info!(scan_id, pid, "attached to a scan another process holds");
                Ok((scan_id, false))
            }
            Err(e) => Err(e),
        }
    }

    /// Joins the scan when this server owns it, else polls its row until the
    /// other process finishes or dies.
    fn wait_for(&self, scan_id: ScanId) -> CoreResult<()> {
        let ours = self
            .scan
            .lock()
            .expect("scan slot poisoned")
            .take_if(|j| j.scan_id == scan_id);
        if let Some(job) = ours {
            return job.join().map(drop);
        }
        let index = self.index()?;
        while index.scan_in_progress()?.is_some_and(|s| s.id == scan_id) {
            std::thread::sleep(Duration::from_millis(500));
        }
        match index.scan(scan_id)?.map(|s| s.status) {
            Some(ScanStatus::Done) => Ok(()),
            _ => Err(CoreError::Invalid(format!(
                "scan {scan_id} did not finish; see `spacehog status`"
            ))),
        }
    }

    fn summary(&self, scan_id: Option<ScanId>) -> CoreResult<Summary> {
        let index = self.index()?;
        let scan = index.require_scan(scan_id)?;
        report::summary(&index, &self.rules, scan, self.config.stale_days)
    }

    fn top(&self, args: TopArgs) -> CoreResult<Listing<report::Item>> {
        let index = self.index()?;
        let scan_id = index.require_scan(None)?;
        let q = TopQuery {
            limit: args.limit.unwrap_or(20),
            category: args.category,
            min_size: args.min_size,
            stale_days: args.stale_days,
            safety: args.safety,
            under: args.under.as_deref().map(resolve),
            kind: args.kind.unwrap_or_default(),
        };
        let items = report::top(&index, &self.rules, scan_id, &q)?;
        Ok(Listing { scan_id, items })
    }

    fn query(&self, args: QueryArgs) -> CoreResult<Listing<NodeView>> {
        let index = self.index()?;
        let scan_id = index.require_scan(None)?;
        let filter = NodeFilter {
            name_glob: args.glob,
            under: args.under.as_deref().map(resolve),
            min_size: args.min_size,
            max_depth: args.max_depth,
            sort: args.sort.unwrap_or_default(),
            limit: args.limit.unwrap_or(20),
            ..Default::default()
        };
        let items = index
            .select(scan_id, &filter)?
            .into_iter()
            .map(|n| NodeView {
                safety: self
                    .rules
                    .effective_safety(&n.path, n.rule_id.as_deref(), n.safety),
                size_human: human::bytes(n.disk_size),
                size: n.disk_size,
                apparent_size: n.apparent_size,
                file_count: n.file_count,
                last_modified: n.max_mtime,
                kind: n.kind,
                bucket: n.bucket,
                rule_id: n.rule_id,
                path: n.path,
            })
            .collect();
        Ok(Listing { scan_id, items })
    }

    fn explain(&self, args: ExplainArgs) -> CoreResult<report::Explanation> {
        report::explain(
            &self.index()?,
            &self.rules,
            &resolve(&args.path),
            self.config.stale_days,
        )
    }

    fn propose(
        &self,
        args: ProposeArgs,
        client: Option<String>,
    ) -> CoreResult<spacehog_core::proposals::ProposeResult> {
        let req = ProposeRequest {
            paths: args.paths.iter().map(|p| resolve(p)).collect(),
            rationale: args.rationale,
            agent: args.agent.or(client),
            mode: args.mode,
        };
        self.index()?
            .propose(&self.rules, &self.config, Actor::Agent, &req)
    }

    fn proposals(&self, status: Option<ProposalStatus>) -> CoreResult<ProposalList> {
        Ok(ProposalList {
            proposals: self.index()?.proposals(status)?,
        })
    }

    fn rule_set(&self) -> RuleList<'_> {
        RuleList {
            buckets: &self.rules.buckets,
            rules: self.rules.rules.iter().map(|r| &r.def).collect(),
        }
    }
}

fn reap(job: ScanJob) {
    let id = job.scan_id;
    match job.join() {
        Ok((scan, timings)) => {
            tracing::info!(scan_id = id, files = scan.files, ?timings, "scan finished")
        }
        Err(e) => tracing::warn!(scan_id = id, error = %e, "scan failed"),
    }
}

/// Expands a leading `~` and canonicalises, so agents can pass either form.
fn resolve(path: &Path) -> PathBuf {
    let expanded = match path.strip_prefix("~") {
        Ok(rest) => paths::home_dir().join(rest),
        Err(_) => path.to_path_buf(),
    };
    spacehog_core::engine::canonical(&expanded)
}

#[derive(Serialize)]
struct ScanView {
    #[serde(flatten)]
    scan: ScanRecord,
    bytes_human: String,
    apparent_human: String,
}

impl From<ScanRecord> for ScanView {
    fn from(scan: ScanRecord) -> Self {
        Self {
            bytes_human: human::bytes(scan.bytes),
            apparent_human: human::bytes(scan.apparent_bytes),
            scan,
        }
    }
}

#[derive(Serialize)]
struct StatusView {
    volumes: Vec<status::Volume>,
    last_scan: Option<ScanView>,
    scan_in_progress: Option<ScanView>,
    pending_proposals: u64,
}

#[derive(Serialize)]
struct ScanReply {
    scan_id: ScanId,
    /// False when the call attached to a scan that was already running.
    started: bool,
    status: ScanStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<Summary>,
}

/// MCP structured content must be an object, so lists travel wrapped.
#[derive(Serialize)]
struct Listing<T> {
    scan_id: ScanId,
    items: Vec<T>,
}

#[derive(Serialize)]
struct NodeView {
    path: String,
    kind: NodeKind,
    size: u64,
    size_human: String,
    apparent_size: u64,
    file_count: u64,
    last_modified: i64,
    bucket: Option<String>,
    rule_id: Option<String>,
    safety: Safety,
}

#[derive(Serialize)]
struct ProposalList {
    proposals: Vec<spacehog_core::proposals::Proposal>,
}

#[derive(Serialize)]
struct RuleList<'a> {
    buckets: &'a [spacehog_core::rules::BucketDef],
    rules: Vec<&'a spacehog_core::rules::RuleDef>,
}

/// The one place core errors become MCP errors. Anything the caller can act
/// on stays a tool result, because clients render protocol errors opaquely
/// and the agent would never read "run a scan first". Only a broken index is
/// a protocol-level internal error.
fn caller_fault(e: &CoreError) -> Option<&'static str> {
    Some(match e {
        CoreError::Sql(_) => return None,
        CoreError::NoScan => "no_scan",
        CoreError::NotIndexed(_) => "not_indexed",
        CoreError::Invalid(_) => "invalid_params",
        CoreError::Refused(_) => "refused",
        CoreError::ScanInProgress { .. } => "scan_in_progress",
        CoreError::Rules(_) | CoreError::Config(_) => "config",
        CoreError::Io(_) => "io",
    })
}

fn tool_error(e: CoreError) -> Result<CallToolResult, ErrorData> {
    match caller_fault(&e) {
        Some(code) => Ok(CallToolResult::structured_error(
            json!({ "error": code, "message": e.to_string() }),
        )),
        None => Err(ErrorData::internal_error(e.to_string(), None)),
    }
}

fn resource_error(e: CoreError) -> ErrorData {
    match caller_fault(&e) {
        Some(code) => ErrorData::invalid_params(e.to_string(), Some(json!({ "error": code }))),
        None => ErrorData::internal_error(e.to_string(), None),
    }
}

async fn blocking<T, F>(state: &Arc<State>, what: &str, f: F) -> Result<CoreResult<T>, ErrorData>
where
    T: Send + 'static,
    F: FnOnce(&State) -> CoreResult<T> + Send + 'static,
{
    let state = state.clone();
    tokio::task::spawn_blocking(move || f(&state))
        .await
        .map_err(|e| ErrorData::internal_error(format!("{what} panicked: {e}"), None))
}

fn to_json<T: Serialize>(value: &T) -> Result<serde_json::Value, ErrorData> {
    serde_json::to_value(value).map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

#[derive(Clone)]
pub(crate) struct Spacehog {
    state: Arc<State>,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

impl Spacehog {
    pub(crate) fn new(state: State) -> Self {
        Self {
            state: Arc::new(state),
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    async fn run<T, F>(&self, tool: &'static str, f: F) -> Result<CallToolResult, ErrorData>
    where
        T: Serialize + Send + 'static,
        F: FnOnce(&State) -> CoreResult<T> + Send + 'static,
    {
        let started = Instant::now();
        let outcome = blocking(&self.state, tool, f).await?;
        let ms = started.elapsed().as_millis() as u64;
        match outcome {
            Ok(value) => {
                tracing::info!(tool, ms, "ok");
                Ok(CallToolResult::structured(to_json(&value)?))
            }
            Err(e) => {
                tracing::warn!(tool, ms, error = %e, "failed");
                tool_error(e)
            }
        }
    }
}

#[tool_router]
impl Spacehog {
    #[tool(
        description = "Disk volumes with free space, the last finished scan, any scan in progress \
                       (with live file and byte counts), and the number of pending cleanup proposals. \
                       Call this first.",
        annotations(read_only_hint = true)
    )]
    async fn status(&self) -> Result<CallToolResult, ErrorData> {
        self.run("status", State::status).await
    }

    #[tool(
        description = "Start a scan that indexes and categorises a directory tree. Returns \
                       `{ scan_id }` at once; poll `status` for progress. With `wait`, blocks and \
                       returns the finished scan's summary. If a scan is already running, returns \
                       that scan instead of starting another.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn scan(
        &self,
        Parameters(args): Parameters<ScanArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("scan", move |s| s.scan(args)).await
    }

    #[tool(
        description = "Bytes per bucket with percentages, stale bytes, and item counts, plus the \
                       ten largest rules. Stale means idle longer than the configured threshold.",
        annotations(read_only_hint = true)
    )]
    async fn summary(
        &self,
        Parameters(args): Parameters<SummaryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("summary", move |s| s.summary(args.scan_id)).await
    }

    #[tool(
        description = "Largest categorised items in the latest scan, ranked by the bytes each one \
                       alone accounts for. Filter by bucket, category, or rule id, by safety class, \
                       by idle days, or by a parent path. Each item carries its safety class, \
                       staleness, project root, and the command that regenerates it.",
        annotations(read_only_hint = true)
    )]
    async fn top(
        &self,
        Parameters(args): Parameters<TopArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("top", move |s| s.top(args)).await
    }

    #[tool(
        description = "Raw index query over every node, categorised or not: filter by a glob on \
                       the node name, a parent path, minimum size, and depth; sort by size or by \
                       oldest modification.",
        annotations(read_only_hint = true)
    )]
    async fn query(
        &self,
        Parameters(args): Parameters<QueryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("query", move |s| s.query(args)).await
    }

    #[tool(
        description = "Why a path is categorised the way it is: the rule that claims it, its \
                       safety class and the reason, how staleness was measured, and its largest \
                       children. Call this on every path before proposing it, and always for \
                       `review` items.",
        annotations(read_only_hint = true)
    )]
    async fn explain(
        &self,
        Parameters(args): Parameters<ExplainArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("explain", move |s| s.explain(args)).await
    }

    #[tool(
        description = "Propose paths for cleanup. Nothing is deleted: this records a pending \
                       proposal for a human to approve. Every path is validated; protected paths, \
                       projects, the home directory, and recent `review` items come back in \
                       `rejected` with a reason. Returns the proposal id, accepted items, total \
                       bytes, and whether a human must approve it.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn propose_cleanup(
        &self,
        Parameters(args): Parameters<ProposeArgs>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = peer.peer_info().map(|i| i.client_info.name.clone());
        self.run("propose_cleanup", move |s| s.propose(args, client))
            .await
    }

    #[tool(
        description = "Cleanup proposals, newest first, with their items, sizes, status, and the \
                       reason a human gave when rejecting one.",
        annotations(read_only_hint = true)
    )]
    async fn list_proposals(
        &self,
        Parameters(args): Parameters<ListProposalsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("list_proposals", move |s| s.proposals(args.status))
            .await
    }

    #[tool(
        description = "The active rule set: bucket definitions and every rule with its matchers, \
                       safety class, staleness basis, and regeneration hint.",
        annotations(read_only_hint = true)
    )]
    async fn rules(&self) -> Result<CallToolResult, ErrorData> {
        let rules = to_json(&self.state.rule_set())?;
        tracing::info!(tool = "rules", "ok");
        Ok(CallToolResult::structured(rules))
    }
}

#[prompt_router]
impl Spacehog {
    #[prompt(
        name = "free-space",
        description = "Free a given number of gigabytes as safely as possible."
    )]
    async fn free_space(&self, Parameters(args): Parameters<FreeSpaceArgs>) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Find the safest way to free {} GB on this machine, prefer regenerable and cache \
                 items, stale first, explain each item, then propose.",
                args.gb.trim()
            ),
        )]
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for Spacehog {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_prompts()
            .build();
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new("spacehog", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(SUMMARY_URI, "summary")
                .with_description("Bucket summary of the latest scan")
                .with_mime_type("application/json"),
            Resource::new(PROPOSALS_URI, "proposals")
                .with_description("Cleanup proposals waiting for approval")
                .with_mime_type("application/json"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri;
        let value = match uri.as_str() {
            SUMMARY_URI => blocking(&self.state, "summary", |s| s.summary(None))
                .await?
                .map_err(resource_error)
                .and_then(|v| to_json(&v)),
            PROPOSALS_URI => blocking(&self.state, "proposals", |s| {
                s.proposals(Some(ProposalStatus::Pending))
            })
            .await?
            .map_err(resource_error)
            .and_then(|v| to_json(&v)),
            _ => Err(ErrorData::resource_not_found(
                format!("no resource {uri}"),
                None,
            )),
        }?;
        let text =
            ResourceContents::text(value.to_string(), uri).with_mime_type("application/json");
        Ok(ReadResourceResult::new(vec![text]).into())
    }
}
