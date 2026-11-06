mod clean;
mod output;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, Subcommand};
use spacehog_core::categorise::Categoriser;
use spacehog_core::changes::{self, Since};
use spacehog_core::config::Config;
use spacehog_core::engine::{ScanRequest, run_scan_timed};
use spacehog_core::proposals::ProposalStatus;
use spacehog_core::query::KindFilter;
use spacehog_core::report::{self, TopQuery};
use spacehog_core::rules::{DEFAULT_RULES, RuleSet, Safety};
use spacehog_core::scan::Progress;
use spacehog_core::{Index, human, status};

/// Find the hog. Feed it to your agent.
#[derive(Parser)]
#[command(name = "spacehog", version, about)]
struct Cli {
    /// Root to scan before opening the TUI.
    path: Option<PathBuf>,
    /// Open the TUI on the Changes view.
    #[arg(long)]
    changes: bool,
    /// Use Nerd Font icons.
    #[arg(long)]
    icons: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Walk a directory tree and write it to the index.
    Scan {
        path: Option<PathBuf>,
        /// Cross filesystem boundaries.
        #[arg(long)]
        all_filesystems: bool,
        #[arg(long)]
        json: bool,
    },
    /// Largest categorised items in the latest scan.
    Top {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Bucket, category, or rule id (e.g. dev-caches, dev-cache, node-modules).
        #[arg(long)]
        category: Option<String>,
        /// Only items idle at least this long, e.g. 90d, 12w, 6m.
        #[arg(long, value_parser = parse_days)]
        stale: Option<i64>,
        #[arg(long, value_delimiter = ',')]
        safety: Option<Vec<String>>,
        #[arg(long)]
        under: Option<PathBuf>,
        #[arg(long)]
        min_size: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// Bytes per bucket and the largest rules.
    Summary {
        #[arg(long)]
        json: bool,
    },
    /// Why a path is categorised the way it is.
    Explain {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Clean categorised items: propose, confirm, and execute in one step.
    Clean {
        /// Bucket, category, or rule id.
        #[arg(long)]
        category: Option<String>,
        #[arg(long, value_parser = parse_days)]
        stale: Option<i64>,
        /// Explicit paths instead of a category query.
        paths: Vec<PathBuf>,
        #[arg(long, default_value_t = 500)]
        limit: usize,
        /// Show what would be cleaned and stop.
        #[arg(long)]
        dry_run: bool,
        /// Delete instead of moving to the Trash. Always asks you to type the total.
        #[arg(long)]
        permanent: bool,
        /// Skip the confirmation prompt (not allowed with --permanent).
        #[arg(long, conflicts_with = "permanent")]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// What grew or shrank between two scans of the same root.
    Changes {
        /// A duration (7d, 2w) or a scan id; defaults to the previous scan.
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Restore everything a cleanup moved to the Trash.
    Undo {
        journal_id: String,
        #[arg(long)]
        json: bool,
    },
    /// The active rule set.
    Rules {
        /// Print the embedded default rules.toml.
        #[arg(long)]
        print_default: bool,
        #[arg(long)]
        json: bool,
    },
    /// Run the MCP server over stdio, or configure a client.
    Mcp {
        #[command(subcommand)]
        action: Option<McpAction>,
    },
    /// List, approve, or reject cleanup proposals.
    Proposals {
        #[command(subcommand)]
        action: Option<ProposalAction>,
        #[arg(long)]
        json: bool,
    },
    /// Disk volumes, the last scan, and pending proposals.
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum McpAction {
    /// Print or write the config snippet for an MCP client.
    Install {
        #[arg(long, value_enum)]
        client: Option<ClientArg>,
        /// Write the config file instead of printing the snippet.
        #[arg(long)]
        write: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ClientArg {
    ClaudeCode,
    ClaudeDesktop,
    Cursor,
}

#[derive(Subcommand)]
enum ProposalAction {
    List {
        #[arg(long)]
        status: Option<String>,
    },
    Approve {
        id: String,
    },
    Reject {
        id: String,
        #[arg(long, default_value = "rejected by user")]
        reason: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("spacehog: {e:#}");
            ExitCode::from(output::exit_code(&e))
        }
    }
}

pub fn parse_days(s: &str) -> std::result::Result<i64, String> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: i64 = num
        .parse()
        .map_err(|_| format!("expected a duration like 90d, got {s}"))?;
    Ok(match unit {
        "" | "d" => n,
        "w" => n * 7,
        "m" | "mo" => n * 30,
        "y" => n * 365,
        _ => return Err(format!("unknown unit {unit}; use d, w, m, or y")),
    })
}

fn load_rules(config: &Config) -> Result<RuleSet> {
    Ok(RuleSet::load(
        Some(&config.rules_path()),
        &spacehog_core::paths::home_dir(),
    )?)
}

fn run(cli: Cli) -> Result<ExitCode> {
    let config = Config::load()?;
    match cli.command {
        None => spacehog_tui::run(spacehog_tui::TuiOptions {
            root: cli.path,
            changes: cli.changes,
            icons: cli.icons || config.icons,
        })?,
        Some(Command::Mcp { action: None }) => spacehog_mcp::serve()?,
        Some(Command::Mcp {
            action: Some(McpAction::Install { client, write }),
        }) => {
            let client = client.map(|c| match c {
                ClientArg::ClaudeCode => spacehog_mcp::Client::ClaudeCode,
                ClientArg::ClaudeDesktop => spacehog_mcp::Client::ClaudeDesktop,
                ClientArg::Cursor => spacehog_mcp::Client::Cursor,
            });
            println!("{}", spacehog_mcp::install(client, write)?);
        }
        Some(Command::Proposals { action, json }) => {
            let index = Index::open_default()?;
            match action.unwrap_or(ProposalAction::List { status: None }) {
                ProposalAction::List { status } => {
                    let status = status
                        .map(|s| {
                            ProposalStatus::parse(&s)
                                .ok_or_else(|| anyhow::anyhow!("unknown status {s}"))
                        })
                        .transpose()?;
                    let list = index.proposals(status)?;
                    if json {
                        output::json(&list)?;
                    } else {
                        output::proposals(&list);
                    }
                }
                ProposalAction::Approve { id } => {
                    index.approve(&id)?;
                    println!("approved {id}");
                }
                ProposalAction::Reject { id, reason } => {
                    index.reject(&id, &reason)?;
                    println!("rejected {id}");
                }
            }
        }
        Some(Command::Scan {
            path,
            all_filesystems,
            json,
        }) => {
            let root = path.unwrap_or_else(|| config.default_root());
            let categoriser = Arc::new(Categoriser::new(Arc::new(load_rules(&config)?)));
            let mut req = ScanRequest::new(root);
            req.options.one_file_system = config.one_file_system && !all_filesystems;
            req.keep = config.keep_scans;
            req.labeler = Some(categoriser);
            let progress = Arc::new(Progress::default());
            let started = Instant::now();
            let show = !json && std::io::stderr().is_terminal();
            let ticker = show.then(|| spawn_ticker(progress.clone()));
            let result = run_scan_timed(&req, progress.clone());
            progress.set_phase(spacehog_core::scan::Phase::Done);
            if let Some(t) = ticker {
                let _ = t.join();
            }
            let (scan, timings) = result?;
            let elapsed = started.elapsed().as_secs_f64();
            if json {
                output::json(
                    &serde_json::json!({ "scan": scan, "elapsed_secs": elapsed, "timings": timings }),
                )?;
            } else {
                println!(
                    "scanned {} in {:.1}s: {} files, {} dirs, {} on disk ({} apparent), {} errors",
                    scan.root,
                    elapsed,
                    scan.files,
                    scan.dirs,
                    human::bytes(scan.bytes),
                    human::bytes(scan.apparent_bytes),
                    scan.errors
                );
            }
        }
        Some(Command::Top {
            limit,
            category,
            stale,
            safety,
            under,
            min_size,
            json,
        }) => {
            let index = Index::open_default()?;
            let rules = load_rules(&config)?;
            let scan = index.require_scan(None)?;
            let safety = safety
                .map(|v| {
                    v.iter()
                        .map(|s| {
                            Safety::parse(s)
                                .ok_or_else(|| anyhow::anyhow!("unknown safety class {s}"))
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?;
            let q = TopQuery {
                limit,
                category,
                min_size,
                stale_days: stale,
                safety,
                under: under.map(|p| spacehog_core::engine::canonical(&p)),
                kind: KindFilter::Any,
            };
            let items = report::top(&index, &rules, scan, &q)?;
            if json {
                output::json(&items)?;
            } else {
                output::item_table(&items);
            }
        }
        Some(Command::Summary { json }) => {
            let index = Index::open_default()?;
            let rules = load_rules(&config)?;
            let scan = index.require_scan(None)?;
            let s = report::summary(&index, &rules, scan, config.stale_days)?;
            if json {
                output::json(&s)?;
            } else {
                output::summary(&s);
            }
        }
        Some(Command::Explain { path, json }) => {
            let index = Index::open_default()?;
            let rules = load_rules(&config)?;
            let e = report::explain(&index, &rules, &path, config.stale_days)?;
            if json {
                output::json(&e)?;
            } else {
                output::explanation(&e);
            }
        }
        Some(Command::Clean {
            category,
            stale,
            paths,
            limit,
            dry_run,
            permanent,
            yes,
            json,
        }) => {
            let index = Index::open_default()?;
            let rules = load_rules(&config)?;
            return clean::run(
                &index,
                &rules,
                &config,
                clean::Args {
                    category,
                    stale,
                    paths,
                    limit,
                    dry_run,
                    permanent,
                    yes,
                    json,
                },
            );
        }
        Some(Command::Changes { since, limit, json }) => {
            let index = Index::open_default()?;
            let rules = load_rules(&config)?;
            let since = match since {
                None => Since::Previous,
                Some(s) if s.chars().all(|c| c.is_ascii_digit()) => Since::Scan(s.parse()?),
                Some(s) => Since::Ago(parse_days(&s).map_err(anyhow::Error::msg)? * 86_400),
            };
            let c = changes::changes(&index, &rules, None, since, limit)?;
            if json {
                output::json(&c)?;
            } else {
                output::changes(&c);
            }
        }
        Some(Command::Undo { journal_id, json }) => {
            let index = Index::open_default()?;
            let restored = index.undo(&journal_id)?;
            if json {
                output::json(&restored)?;
            } else {
                for r in &restored {
                    let status = match r.outcome {
                        spacehog_core::cleanup::Outcome::Restored => "restored",
                        _ => "failed",
                    };
                    println!(
                        "{status:>9}  {:>10}  {}  {}",
                        r.size_human,
                        r.path,
                        r.reason.as_deref().unwrap_or("")
                    );
                }
            }
        }
        Some(Command::Rules {
            print_default,
            json,
        }) => {
            if print_default {
                print!("{DEFAULT_RULES}");
            } else {
                let rules = load_rules(&config)?;
                let defs: Vec<_> = rules.rules.iter().map(|r| &r.def).collect();
                if json {
                    output::json(&serde_json::json!({ "buckets": rules.buckets, "rules": defs }))?;
                } else {
                    for r in &rules.rules {
                        println!(
                            "{:<26} {:<12} {:<12} {}",
                            r.id,
                            r.bucket,
                            r.safety().as_str(),
                            r.def.label
                        );
                    }
                }
            }
        }
        Some(Command::Status { json }) => {
            let index = Index::open_default()?;
            let s = status::status(&index)?;
            if json {
                output::json(&s)?;
            } else {
                output::status(&s);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn spawn_ticker(progress: Arc<Progress>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use spacehog_core::scan::Phase;
        while progress.phase() != Phase::Done {
            let p = progress.snapshot();
            match p.phase {
                Phase::Walking => eprint!(
                    "\r\x1b[2K{} files  {} dirs  {}  {}",
                    p.files,
                    p.dirs,
                    human::bytes(p.bytes),
                    output::truncate_middle(&p.current_path, 60)
                ),
                Phase::Labelling => eprint!("\r\x1b[2Kcategorising {} files", p.files),
                Phase::Writing => eprint!(
                    "\r\x1b[2Kindexing {} / {} rows",
                    p.written,
                    p.files + p.dirs
                ),
                Phase::Done => {}
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprint!("\r\x1b[2K");
    })
}
