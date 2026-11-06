use std::io::Write;

use serde::Serialize;
use spacehog_core::human;
use spacehog_core::proposals::Proposal;
use spacehog_core::report::{Explanation, Item, Summary};
use spacehog_core::status::Status;

/// Exit codes: 0 ok, 1 error, 2 usage (clap), 3 no index yet, 4 refused by a safety rule,
/// 5 a scan is already running.
pub fn exit_code(e: &anyhow::Error) -> u8 {
    match e.downcast_ref::<spacehog_core::Error>() {
        Some(spacehog_core::Error::NoScan) => 3,
        Some(spacehog_core::Error::Refused(_)) => 4,
        Some(spacehog_core::Error::ScanInProgress { .. }) => 5,
        _ => 1,
    }
}

pub fn json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}

pub fn item_table(items: &[Item]) {
    for i in items {
        let stale = i.stale_days.map(human::days).unwrap_or_default();
        println!(
            "{:>10}  {} {:<12} {:>5}  {}",
            i.size_human,
            i.safety.glyph(),
            i.bucket,
            stale,
            i.path
        );
    }
}

pub fn summary(s: &Summary) {
    println!("{} scanned in {}", s.total_human, s.scan.root);
    for b in &s.buckets {
        let bar = "█".repeat(((b.pct / 100.0) * 40.0).round() as usize);
        println!("{:<15} {:>10} {:>5.1}%  {}", b.label, b.human, b.pct, bar);
    }
    println!("stale (>{}d): {}", s.stale_days, s.stale_human);
    println!();
    for r in &s.top_rules {
        println!(
            "{:<28} {:>10} {:>6} items  {}",
            r.label,
            r.human,
            r.items,
            r.safety.as_str()
        );
    }
}

pub fn explanation(e: &Explanation) {
    let i = &e.item;
    println!("{}", i.path);
    println!(
        "  size       {} ({} apparent)",
        i.size_human,
        human::bytes(i.apparent_size)
    );
    println!("  bucket     {}", i.bucket_label);
    if let Some(rule) = &e.rule {
        println!("  rule       {} ({})", rule.id, rule.label);
    }
    if let Some(c) = &e.claimed_by
        && c != &i.path
    {
        println!("  claimed by {c}");
    }
    println!("  safety     {}", e.safety_reason);
    println!("  staleness  {}", e.staleness);
    if let Some(r) = &i.regen_hint {
        println!("  regen      {r}");
    }
    if !e.children.is_empty() {
        println!("  largest children:");
        for c in &e.children {
            println!(
                "    {:>10}  {}",
                c.size_human,
                c.path.rsplit('/').next().unwrap_or(&c.path)
            );
        }
    }
}

pub fn status(s: &Status) {
    for v in &s.volumes {
        println!(
            "{:<24} {:>10} used of {:>10} ({:.1}%), {} free",
            v.mount, v.used_human, v.total_human, v.used_pct, v.free_human
        );
    }
    match &s.last_scan {
        Some(scan) => println!(
            "last scan: {} ({} files, {})",
            scan.root,
            scan.files,
            human::bytes(scan.bytes)
        ),
        None => println!("last scan: none"),
    }
    if let Some(p) = &s.scan_in_progress {
        println!("scan running: {} ({} files so far)", p.root, p.files);
    }
    println!("pending proposals: {}", s.pending_proposals);
}

pub fn truncate_middle(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let keep = (max - 1) / 2;
    let head: String = chars[..keep].iter().collect();
    let tail: String = chars[chars.len() - (max - 1 - keep)..].iter().collect();
    format!("{head}…{tail}")
}

pub fn proposals(list: &[Proposal]) {
    if list.is_empty() {
        println!("no proposals");
    }
    for p in list {
        println!(
            "{}  {:<9} {:>10}  {} items  from {}: {}",
            p.id,
            p.status.as_str(),
            p.total_human,
            p.items.len(),
            p.agent,
            p.rationale
        );
        if let Some(r) = &p.reason {
            println!("    reason: {r}");
        }
    }
}

pub fn changes(c: &spacehog_core::changes::Changes) {
    println!(
        "{} in {} since scan {}",
        c.total_delta_human, c.root, c.from.id
    );
    for b in &c.buckets {
        println!("  {:<15} {:>11}", b.label, b.delta_human);
    }
    if !c.grown.is_empty() {
        println!("grew:");
        for d in &c.grown {
            println!("  {:>11}  {}", d.delta_human, d.path);
        }
    }
    if !c.shrunk.is_empty() {
        println!("shrank:");
        for d in &c.shrunk {
            println!("  {:>11}  {}", d.delta_human, d.path);
        }
    }
}
