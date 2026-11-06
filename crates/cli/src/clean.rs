use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use spacehog_core::Index;
use spacehog_core::cleanup::{ExecuteRequest, Outcome, Trash};
use spacehog_core::config::{Config, DeleteMode};
use spacehog_core::proposals::{Actor, ProposeRequest};
use spacehog_core::report::{self, TopQuery};
use spacehog_core::rules::{RuleSet, Safety};

use crate::output;

pub struct Args {
    pub category: Option<String>,
    pub stale: Option<i64>,
    pub paths: Vec<PathBuf>,
    pub limit: usize,
    pub dry_run: bool,
    pub permanent: bool,
    pub yes: bool,
    pub json: bool,
}

pub fn run(index: &Index, rules: &RuleSet, config: &Config, a: Args) -> Result<ExitCode> {
    if a.paths.is_empty() && a.category.is_none() && a.stale.is_none() {
        bail!("name paths, or narrow with --category and/or --stale");
    }
    let paths: Vec<PathBuf> = if a.paths.is_empty() {
        let scan = index.require_scan(None)?;
        let q = TopQuery {
            limit: a.limit,
            category: a.category.clone(),
            stale_days: a.stale,
            safety: Some(vec![Safety::Regenerable, Safety::Cache, Safety::Review]),
            ..Default::default()
        };
        report::top(index, rules, scan, &q)?
            .into_iter()
            .map(|i| PathBuf::from(i.path))
            .collect()
    } else {
        a.paths
    };
    if paths.is_empty() {
        println!("nothing matches");
        return Ok(ExitCode::SUCCESS);
    }
    let mode = if a.permanent {
        DeleteMode::Permanent
    } else {
        config.cleanup.mode
    };
    let proposal = index.propose(
        rules,
        config,
        Actor::User,
        &ProposeRequest {
            paths,
            rationale: "spacehog clean".into(),
            agent: Some("cli".into()),
            mode: Some(mode),
        },
    )?;
    if !a.json || a.dry_run {
        for i in &proposal.accepted {
            let stale = i
                .stale_days
                .map(spacehog_core::human::days)
                .unwrap_or_default();
            println!(
                "{:>10}  {} {:<12} {:>5}  {}",
                i.size_human,
                i.safety.glyph(),
                i.bucket,
                stale,
                i.path
            );
        }
        for r in &proposal.rejected {
            println!("   skipped  {}  ({})", r.path, r.reason);
        }
    }
    let Some(id) = proposal.proposal_id.clone() else {
        println!("nothing can be cleaned");
        return Ok(ExitCode::from(4));
    };
    if a.dry_run {
        index.reject(&id, "dry run")?;
        println!("would free {} ({})", proposal.total_human, mode.as_str());
        return Ok(ExitCode::SUCCESS);
    }
    let review = proposal
        .accepted
        .iter()
        .filter(|i| i.safety == Safety::Review)
        .count();
    let confirmed = if a.yes {
        true
    } else {
        prompt(&proposal.total_human, mode, review)?
    };
    if !confirmed {
        index.reject(&id, "declined at the prompt")?;
        println!("nothing changed");
        return Ok(ExitCode::SUCCESS);
    }
    let report = index.execute(
        rules,
        config,
        &Trash::platform_default(),
        &ExecuteRequest {
            proposal_id: &id,
            confirm: &proposal.total_human,
            actor: Actor::User,
        },
    )?;
    if a.json {
        output::json(&report)?;
    } else {
        for r in report.results.iter().filter(|r| !r.outcome.freed()) {
            println!(
                "{:>9}  {}  ({})",
                outcome(r.outcome),
                r.path,
                r.reason.as_deref().unwrap_or("")
            );
        }
        println!(
            "freed {} · journal {}",
            report.bytes_freed_human, report.journal_id
        );
        if mode == DeleteMode::Trash {
            println!("undo with: spacehog undo {}", report.journal_id);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn outcome(o: Outcome) -> &'static str {
    match o {
        Outcome::Trashed => "trashed",
        Outcome::Deleted => "deleted",
        Outcome::Skipped => "skipped",
        Outcome::Failed => "failed",
        Outcome::Restored => "restored",
    }
}

/// Trash asks y/N. Permanent makes you type the total, e.g. `31.2 GB`.
fn prompt(total: &str, mode: DeleteMode, review: usize) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("refusing to clean without a terminal; pass --yes (trash only)");
    }
    let mut out = std::io::stderr();
    if review > 0 {
        writeln!(
            out,
            "{review} item(s) are marked review: they may hold your work."
        )?;
    }
    match mode {
        DeleteMode::Trash => write!(out, "Move {total} to the Trash? [y/N] ")?,
        DeleteMode::Permanent => write!(
            out,
            "Permanently delete {total}? Type the total ({total}) to confirm: "
        )?,
    }
    out.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let line = line.trim();
    Ok(match mode {
        DeleteMode::Trash => matches!(line, "y" | "Y" | "yes"),
        DeleteMode::Permanent => line == total,
    })
}
