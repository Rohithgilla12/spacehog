use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Gauge, Paragraph, Wrap};
use spacehog_core::Index;
use spacehog_core::cleanup::{ExecuteRequest, ExecutionReport, Trash};
use spacehog_core::config::DeleteMode;
use spacehog_core::human;
use spacehog_core::proposals::{
    Actor, Proposal, ProposalItem, ProposalStatus, ProposeRequest, Rejection,
};
use spacehog_core::report::Item;
use spacehog_core::rules::{RuleSet, Safety};
use tui_big_text::{BigText, PixelSize};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::data::{self, Ctx, truncate_middle};
use crate::keys::Action;
use crate::theme::Theme;
use crate::widgets;

const COUNTER: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Queue,
    Proposals,
}

/// One thing a cleanup would remove, with what a person needs to judge it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub bucket: Option<String>,
    pub safety: Safety,
    pub rule_label: Option<String>,
    pub stale_days: Option<i64>,
    pub regen_hint: Option<String>,
}

impl Entry {
    pub fn from_item(i: &Item) -> Self {
        Self {
            path: i.path.clone(),
            size: i.size,
            bucket: Some(i.bucket.clone()),
            safety: i.safety,
            rule_label: i.rule_label.clone(),
            stale_days: i.stale_days,
            regen_hint: i.regen_hint.clone(),
        }
    }

    fn from_proposal(i: &ProposalItem, rules: &RuleSet) -> Self {
        Self {
            path: i.path.clone(),
            size: i.size,
            bucket: i.bucket.clone(),
            safety: i.safety,
            rule_label: i
                .rule_id
                .as_deref()
                .and_then(|id| rules.get(id))
                .map(|r| r.def.label.clone()),
            stale_days: i.stale_days,
            regen_hint: i.regen_hint.clone(),
        }
    }

    /// One line on why removing this is safe, from its safety class, the
    /// rule that claimed it, and how long it has been idle.
    pub fn why_safe(&self, stale_threshold: i64) -> String {
        let what = self.rule_label.as_deref().unwrap_or("this item");
        let base = match self.safety {
            Safety::Regenerable => format!("{what} is rebuilt by its tool"),
            Safety::Cache => format!("{what} is a cache that refills on demand"),
            Safety::Review => format!("{what} may hold your own data; check it first"),
            Safety::Protected => "protected: spacehog never cleans this".into(),
        };
        match self.stale_days {
            Some(d) if d >= stale_threshold => format!("{base}; idle {}", human::days(d)),
            Some(0) => format!("{base}; used today"),
            Some(d) => format!("{base}; last used {}", human::days(d)),
            None => base,
        }
    }
}

/// Item count, bytes, and the mix of safety classes, as the footers spell it:
/// `3 items · 1.2 GB · all regenerable` or `… · 2 regenerable, 1 cache`.
pub fn describe(items: impl IntoIterator<Item = (u64, Safety)>) -> String {
    let mut count = 0;
    let mut bytes = 0;
    let mut classes: Vec<(Safety, usize)> = Vec::new();
    for (size, safety) in items {
        count += 1;
        bytes += size;
        match classes.iter_mut().find(|(s, _)| *s == safety) {
            Some((_, n)) => *n += 1,
            None => classes.push((safety, 1)),
        }
    }
    classes.sort();
    let mix = match classes.as_slice() {
        [] => String::new(),
        [(s, _)] => format!(" · all {}", s.as_str()),
        many => format!(
            " · {}",
            many.iter()
                .map(|(s, n)| format!("{n} {}", s.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    format!(
        "{count} item{} · {}{mix}",
        if count == 1 { "" } else { "s" },
        human::bytes(bytes)
    )
}

/// What `D` acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Queue,
    Proposal(String),
}

/// A validated proposal, ready to hand to the executor.
#[derive(Debug, Clone)]
pub struct Ready {
    pub id: String,
    pub target: Target,
    pub agent: String,
    pub total_human: String,
    pub mode: DeleteMode,
    pub entries: Vec<Entry>,
    /// Paths `propose` refused, with why. Only the queue has these.
    pub rejected: Vec<Rejection>,
}

/// One per-item outcome, read from the journal while the executor runs.
#[derive(Debug, Clone)]
pub struct Streamed {
    pub path: String,
    pub size: u64,
    pub outcome: String,
    pub error: Option<String>,
}

impl Streamed {
    fn freed(&self) -> bool {
        matches!(self.outcome.as_str(), "trashed" | "deleted")
    }
}

/// A number that eases toward its target, for the bytes-freed counter.
#[derive(Debug, Clone, Copy)]
pub struct Counter {
    from: u64,
    to: u64,
    since: Instant,
    pub shown: u64,
}

impl Counter {
    fn new() -> Self {
        Self {
            from: 0,
            to: 0,
            since: Instant::now(),
            shown: 0,
        }
    }

    fn aim(&mut self, to: u64) {
        if to != self.to {
            self.from = self.shown;
            self.to = to;
            self.since = Instant::now();
        }
    }

    fn tick(&mut self) -> bool {
        let raw = (self.since.elapsed().as_secs_f64() / COUNTER.as_secs_f64()).min(1.0);
        let eased = 1.0 - (1.0 - raw).powi(3);
        let next = self.from as f64 + (self.to as f64 - self.from as f64) * eased;
        let next = if raw >= 1.0 { self.to } else { next as u64 };
        let changed = next != self.shown;
        self.shown = next;
        changed
    }

    pub fn settled(&self) -> bool {
        self.shown == self.to
    }
}

impl CleanupState {
    /// Blocks until a running executor returns, so quitting never abandons
    /// an item half-moved or leaves a run out of the journal's reach.
    pub fn finish(&mut self) {
        if let Phase::Running(run) = std::mem::replace(&mut self.phase, Phase::Browse) {
            let _ = run.job.join();
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running(_))
    }
}

pub struct Run {
    pub ready: Ready,
    job: JoinHandle<std::result::Result<ExecutionReport, String>>,
    /// Journal rows up to this id belong to earlier runs.
    after: i64,
    pub streamed: Vec<Streamed>,
    pub counter: Counter,
}

pub struct Done {
    pub ready: Ready,
    pub outcome: std::result::Result<ExecutionReport, String>,
    pub counter: Counter,
    pub undone: Option<String>,
}

/// Where the Cleanup screen is. Each phase owns exactly the data it needs,
/// so a reject reason cannot outlive its prompt and a run cannot start twice.
pub enum Phase {
    Browse,
    /// Typing why an agent's proposal is rejected; the agent reads it back.
    Rejecting {
        id: String,
        reason: Input,
    },
    /// Asking about each `review` item before anything is proposed or run.
    Reviewing {
        target: Target,
        entries: Vec<Entry>,
        at: usize,
        declined: Vec<usize>,
    },
    /// Permanent mode: the exact total must be typed before the run starts.
    Confirming {
        ready: Ready,
        typed: Input,
        wrong: bool,
    },
    /// Nothing in the queue survived validation.
    Refused(Vec<Rejection>),
    Running(Run),
    Done(Done),
}

pub struct CleanupState {
    pub tab: Tab,
    pub queue: Vec<Entry>,
    pub proposals: Vec<Proposal>,
    pub cursor: usize,
    pub mode: DeleteMode,
    pub phase: Phase,
    pub last_journal: Option<String>,
    scroll: usize,
    /// Screen rows of the list from the last draw, for mouse clicks.
    pub rows: Vec<(Rect, usize)>,
}

impl CleanupState {
    pub fn new(mode: DeleteMode) -> Self {
        Self {
            tab: Tab::Queue,
            queue: Vec::new(),
            proposals: Vec::new(),
            cursor: 0,
            mode,
            phase: Phase::Browse,
            last_journal: None,
            scroll: 0,
            rows: Vec::new(),
        }
    }

    /// Rebuilds queue rows from the paths the views queued.
    pub fn load_queue(&mut self, ctx: &Ctx, queue: &[PathBuf]) -> Result<()> {
        self.queue = queue
            .iter()
            .filter_map(|p| ctx.index.find_in_latest(p).transpose())
            .map(|n| n.map(|n| Entry::from_item(&Item::from_node(&ctx.rules, &n))))
            .collect::<spacehog_core::Result<_>>()?;
        self.clamp();
        Ok(())
    }

    /// Re-reads open proposals, keeping the cursor on the same one.
    pub fn load_proposals(&mut self, ctx: &Ctx) -> Result<()> {
        let keep = self.selected_proposal().map(|p| p.id.clone());
        self.proposals = ctx
            .index
            .proposals(None)?
            .into_iter()
            .filter(|p| matches!(p.status, ProposalStatus::Pending | ProposalStatus::Approved))
            .collect();
        if self.tab == Tab::Proposals
            && let Some(i) = keep.and_then(|id| self.proposals.iter().position(|p| p.id == id))
        {
            self.cursor = i;
        }
        self.clamp();
        Ok(())
    }

    fn len(&self) -> usize {
        match self.tab {
            Tab::Queue => self.queue.len(),
            Tab::Proposals => self.proposals.len(),
        }
    }

    fn clamp(&mut self) {
        self.cursor = self.cursor.min(self.len().saturating_sub(1));
    }

    pub fn selected_proposal(&self) -> Option<&Proposal> {
        match self.tab {
            Tab::Proposals => self.proposals.get(self.cursor),
            Tab::Queue => None,
        }
    }

    /// The path `i` explains: the queue row, or a proposal's largest item.
    pub fn selected_path(&self) -> Option<PathBuf> {
        match self.tab {
            Tab::Queue => self.queue.get(self.cursor).map(|e| PathBuf::from(&e.path)),
            Tab::Proposals => self
                .selected_proposal()
                .and_then(|p| p.items.first())
                .map(|i| PathBuf::from(&i.path)),
        }
    }

    /// Whether a prompt or a run owns the keyboard instead of the key table.
    pub fn captures_keys(&self) -> bool {
        !matches!(self.phase, Phase::Browse)
    }

    pub fn animating(&self) -> bool {
        match &self.phase {
            Phase::Running(_) => true,
            Phase::Done(d) => !d.counter.settled(),
            _ => false,
        }
    }

    pub fn set_cursor(&mut self, i: usize) {
        self.cursor = i;
        self.clamp();
    }

    /// Handles a key-table action while browsing. Returns a status message.
    pub fn act(
        &mut self,
        action: Action,
        ctx: &Ctx,
        queue: &mut Vec<PathBuf>,
    ) -> Result<Option<String>> {
        match action {
            Action::Up => self.set_cursor(self.cursor.saturating_sub(1)),
            Action::Down => self.set_cursor(self.cursor + 1),
            Action::PrevTab | Action::NextTab => {
                self.tab = if action == Action::PrevTab {
                    Tab::Queue
                } else {
                    Tab::Proposals
                };
                self.cursor = 0;
                self.scroll = 0;
            }
            Action::Approve => {
                let Some(p) = self.selected_proposal() else {
                    return Ok(Some(
                        "enter approves an agent's proposal; D cleans your queue".into(),
                    ));
                };
                let id = p.id.clone();
                let msg = match ctx.index.approve(&id) {
                    Ok(()) => format!("approved {id}; D runs it now, or the agent can"),
                    Err(e) => e.to_string(),
                };
                self.load_proposals(ctx)?;
                return Ok(Some(msg));
            }
            Action::Reject => match self.tab {
                Tab::Proposals => {
                    if let Some(p) = self.selected_proposal() {
                        self.phase = Phase::Rejecting {
                            id: p.id.clone(),
                            reason: Input::default(),
                        };
                    }
                }
                Tab::Queue => {
                    if let Some(e) = self.queue.get(self.cursor) {
                        let msg = format!("dropped {} from the queue", ctx.tilde(&e.path));
                        queue.retain(|p| p.as_os_str() != e.path.as_str());
                        self.load_queue(ctx, queue)?;
                        return Ok(Some(msg));
                    }
                }
            },
            Action::ToggleMode => {
                self.mode = match self.mode {
                    DeleteMode::Trash => DeleteMode::Permanent,
                    DeleteMode::Permanent => DeleteMode::Trash,
                };
                return Ok(Some(match self.mode {
                    DeleteMode::Trash => "queue goes to the Trash; u undoes it".into(),
                    DeleteMode::Permanent => {
                        "permanent: the queue is deleted for good once you type its total".into()
                    }
                }));
            }
            Action::Execute => return self.start(ctx),
            Action::Undo => {
                let Some(j) = self.last_journal.clone() else {
                    return Ok(Some("nothing to undo yet".into()));
                };
                return Ok(Some(undo(ctx, &j)));
            }
            _ => {}
        }
        Ok(None)
    }

    /// `D`: gathers the entries, then walks the review, propose, confirm,
    /// and run steps, each only when it applies.
    fn start(&mut self, ctx: &Ctx) -> Result<Option<String>> {
        let (target, entries) = match self.tab {
            Tab::Queue => (Target::Queue, self.queue.clone()),
            Tab::Proposals => match self.selected_proposal() {
                Some(p) => (
                    Target::Proposal(p.id.clone()),
                    p.items
                        .iter()
                        .map(|i| Entry::from_proposal(i, &ctx.rules))
                        .collect(),
                ),
                None => (Target::Proposal(String::new()), Vec::new()),
            },
        };
        if entries.is_empty() {
            return Ok(Some("nothing to clean".into()));
        }
        match entries.iter().position(|e| e.safety == Safety::Review) {
            Some(at) => {
                self.phase = Phase::Reviewing {
                    target,
                    entries,
                    at,
                    declined: Vec::new(),
                };
                Ok(None)
            }
            None => self.prepare(ctx, target, entries, &[]),
        }
    }

    /// Turns reviewed entries into a validated proposal. The queue is
    /// proposed as the user now; an agent's proposal is used as it stands.
    fn prepare(
        &mut self,
        ctx: &Ctx,
        target: Target,
        entries: Vec<Entry>,
        declined: &[usize],
    ) -> Result<Option<String>> {
        self.phase = Phase::Browse;
        let ready = match &target {
            Target::Queue => {
                let paths: Vec<PathBuf> = entries
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !declined.contains(i))
                    .map(|(_, e)| PathBuf::from(&e.path))
                    .collect();
                if paths.is_empty() {
                    return Ok(Some("nothing left to clean".into()));
                }
                let res = ctx.index.propose(
                    &ctx.rules,
                    &ctx.config,
                    Actor::User,
                    &ProposeRequest {
                        paths,
                        rationale: "Queued in the spacehog TUI".into(),
                        agent: None,
                        mode: Some(self.mode),
                    },
                )?;
                let Some(id) = res.proposal_id else {
                    self.phase = Phase::Refused(res.rejected);
                    return Ok(None);
                };
                Ready {
                    id,
                    target,
                    agent: "you".into(),
                    total_human: res.total_human,
                    mode: res.mode,
                    entries: res.accepted.iter().map(Entry::from_item).collect(),
                    rejected: res.rejected,
                }
            }
            Target::Proposal(id) => {
                if !declined.is_empty() {
                    return Ok(Some(format!(
                        "kept {} review item{}, so nothing ran; x rejects the proposal with a reason",
                        declined.len(),
                        if declined.len() == 1 { "" } else { "s" }
                    )));
                }
                let Some(p) = ctx.index.proposal(id)? else {
                    return Ok(Some(format!("proposal {id} is gone")));
                };
                Ready {
                    id: p.id.clone(),
                    target: target.clone(),
                    agent: p.agent.clone(),
                    total_human: p.total_human.clone(),
                    mode: p.mode,
                    entries,
                    rejected: Vec::new(),
                }
            }
        };
        if ready.mode == DeleteMode::Permanent {
            self.phase = Phase::Confirming {
                ready,
                typed: Input::default(),
                wrong: false,
            };
        } else {
            self.run(ctx, ready);
        }
        Ok(None)
    }

    /// Runs the executor on its own thread with its own connection, since
    /// rusqlite connections are not `Sync`.
    fn run(&mut self, ctx: &Ctx, ready: Ready) {
        let db = ctx.db.clone();
        let rules = ctx.rules.clone();
        let config = ctx.config.clone();
        let id = ready.id.clone();
        let confirm = ready.total_human.clone();
        let after = ctx
            .index
            .connection()
            .query_row("SELECT COALESCE(MAX(id), 0) FROM journal", [], |r| r.get(0))
            .unwrap_or(0);
        let job = std::thread::spawn(move || {
            let index = Index::open(&db).map_err(|e| e.to_string())?;
            index
                .execute(
                    &rules,
                    &config,
                    &Trash::platform_default(),
                    &ExecuteRequest {
                        proposal_id: &id,
                        confirm: &confirm,
                        actor: Actor::User,
                    },
                )
                .map_err(|e| e.to_string())
        });
        self.phase = Phase::Running(Run {
            ready,
            job,
            after,
            streamed: Vec::new(),
            counter: Counter::new(),
        });
    }

    /// Keys while a prompt or a run owns the keyboard.
    pub fn key(&mut self, key: KeyEvent, ctx: &Ctx, queue: &[PathBuf]) -> Result<Option<String>> {
        let phase = std::mem::replace(&mut self.phase, Phase::Browse);
        match phase {
            Phase::Browse => Ok(None),
            Phase::Rejecting { id, mut reason } => match key.code {
                KeyCode::Esc => Ok(None),
                KeyCode::Enter => {
                    let why = reason.value().trim();
                    let why = if why.is_empty() {
                        "rejected in the spacehog TUI"
                    } else {
                        why
                    };
                    let msg = match ctx.index.reject(&id, why) {
                        Ok(()) => format!("rejected {id}; the agent sees: {why}"),
                        Err(e) => e.to_string(),
                    };
                    self.load_proposals(ctx)?;
                    Ok(Some(msg))
                }
                _ => {
                    reason.handle_event(&Event::Key(key));
                    self.phase = Phase::Rejecting { id, reason };
                    Ok(None)
                }
            },
            Phase::Reviewing {
                target,
                entries,
                at,
                mut declined,
            } => {
                match key.code {
                    KeyCode::Esc => return Ok(Some("cleanup cancelled".into())),
                    KeyCode::Char('y') => {}
                    KeyCode::Char('n') => declined.push(at),
                    _ => {
                        self.phase = Phase::Reviewing {
                            target,
                            entries,
                            at,
                            declined,
                        };
                        return Ok(None);
                    }
                }
                let next = entries
                    .iter()
                    .enumerate()
                    .skip(at + 1)
                    .find(|(_, e)| e.safety == Safety::Review)
                    .map(|(i, _)| i);
                match next {
                    Some(at) => {
                        self.phase = Phase::Reviewing {
                            target,
                            entries,
                            at,
                            declined,
                        };
                        Ok(None)
                    }
                    None => self.prepare(ctx, target, entries, &declined),
                }
            }
            Phase::Confirming {
                ready,
                mut typed,
                wrong,
            } => match key.code {
                KeyCode::Esc => {
                    // An approved leftover would let anyone holding the total run it.
                    if ready.target == Target::Queue {
                        ctx.index.reject(&ready.id, "cancelled in the TUI")?;
                    }
                    Ok(Some("permanent delete cancelled".into()))
                }
                KeyCode::Enter if typed.value().trim() == ready.total_human => {
                    self.run(ctx, ready);
                    Ok(None)
                }
                KeyCode::Enter => {
                    self.phase = Phase::Confirming {
                        ready,
                        typed,
                        wrong: true,
                    };
                    Ok(None)
                }
                _ => {
                    typed.handle_event(&Event::Key(key));
                    self.phase = Phase::Confirming {
                        ready,
                        typed,
                        wrong: wrong && key.code != KeyCode::Backspace,
                    };
                    Ok(None)
                }
            },
            Phase::Refused(r) => {
                if !matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                    self.phase = Phase::Refused(r);
                }
                Ok(None)
            }
            Phase::Running(run) => {
                self.phase = Phase::Running(run);
                Ok(None)
            }
            Phase::Done(mut done) => match key.code {
                KeyCode::Char('u') => {
                    let msg = match &done.outcome {
                        Ok(r) => undo(ctx, &r.journal_id),
                        Err(_) => "nothing ran, so there is nothing to undo".into(),
                    };
                    done.undone = Some(msg.clone());
                    self.phase = Phase::Done(done);
                    Ok(Some(msg))
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.load_queue(ctx, queue)?;
                    self.load_proposals(ctx)?;
                    Ok(None)
                }
                _ => {
                    self.phase = Phase::Done(done);
                    Ok(None)
                }
            },
        }
    }

    /// Streams journal rows while the executor runs and animates the
    /// counter. Returns (redraw, finished) where finished means the index
    /// changed and views should reload.
    pub fn tick(&mut self, ctx: &Ctx, queue: &mut Vec<PathBuf>) -> Result<(bool, bool)> {
        match &mut self.phase {
            Phase::Running(run) => {
                run.streamed = journal_rows(&ctx.index, &run.ready.id, run.after)?;
                run.counter.aim(
                    run.streamed
                        .iter()
                        .filter(|s| s.freed())
                        .map(|s| s.size)
                        .sum(),
                );
                run.counter.tick();
                if !run.job.is_finished() {
                    return Ok((true, false));
                }
                let Phase::Running(run) = std::mem::replace(&mut self.phase, Phase::Browse) else {
                    unreachable!("matched Running above");
                };
                let outcome = run
                    .job
                    .join()
                    .unwrap_or_else(|_| Err("the cleanup thread panicked".into()));
                let mut counter = run.counter;
                if let Ok(report) = &outcome {
                    counter.aim(report.bytes_freed);
                    self.last_journal = Some(report.journal_id.clone());
                    if run.ready.target == Target::Queue {
                        let freed: Vec<&str> = report
                            .results
                            .iter()
                            .filter(|r| r.outcome.freed())
                            .map(|r| r.path.as_str())
                            .collect();
                        queue.retain(|p| !freed.iter().any(|f| Path::new(f) == p));
                    }
                }
                self.phase = Phase::Done(Done {
                    ready: run.ready,
                    outcome,
                    counter,
                    undone: None,
                });
                Ok((true, true))
            }
            Phase::Done(done) => Ok((done.counter.tick(), false)),
            _ => Ok((false, false)),
        }
    }
}

/// Per-item outcomes this run has journalled so far. A run interrupted by
/// quitting leaves its proposal runnable, so earlier rows are skipped by id.
fn journal_rows(index: &Index, proposal: &str, after: i64) -> Result<Vec<Streamed>> {
    let mut stmt = index.connection().prepare_cached(
        "SELECT path, size, outcome, error FROM journal WHERE proposal_id = ?1 AND id > ?2 ORDER BY id",
    )?;
    let rows = stmt.query_map(rusqlite::params![proposal, after], |r| {
        Ok(Streamed {
            path: r.get(0)?,
            size: r.get::<_, i64>(1)? as u64,
            outcome: r.get(2)?,
            error: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn undo(ctx: &Ctx, journal: &str) -> String {
    match ctx.index.undo(journal) {
        Ok(results) => {
            let back = results
                .iter()
                .filter(|r| r.outcome == spacehog_core::cleanup::Outcome::Restored)
                .count();
            let failed = results.len() - back;
            if failed == 0 {
                format!("restored {back} item{}", if back == 1 { "" } else { "s" })
            } else {
                format!("restored {back}, {failed} could not be restored")
            }
        }
        Err(e) => e.to_string(),
    }
}

pub struct Env<'a> {
    pub ctx: &'a Ctx,
    pub theme: &'a Theme,
}

pub fn render(frame: &mut Frame, area: Rect, s: &mut CleanupState, env: &Env) {
    match &s.phase {
        Phase::Running(_) | Phase::Done(_) => {
            render_run(frame, area, s, env);
            return;
        }
        _ => {}
    }
    let t = env.theme;
    let [tabs, body, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let tab = |label: String, on: bool| {
        Span::styled(
            format!(" {label} "),
            if on {
                t.selected().fg(t.accent())
            } else {
                t.dim()
            },
        )
    };
    let mode = match s.mode {
        DeleteMode::Trash => Span::styled("mode: Trash ", t.dim()),
        DeleteMode::Permanent => Span::styled(
            "mode: PERMANENT ",
            Style::new().fg(t.warn()).add_modifier(Modifier::BOLD),
        ),
    };
    let mode_w = mode.width() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(mode_w)]).areas(tabs);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            tab(format!("Queue ({})", s.queue.len()), s.tab == Tab::Queue),
            Span::styled("│", t.border()),
            tab(
                format!("Proposals ({})", s.proposals.len()),
                s.tab == Tab::Proposals,
            ),
        ])),
        l,
    );
    frame.render_widget(Paragraph::new(mode), r);

    let empty = match s.tab {
        Tab::Queue => s.queue.is_empty(),
        Tab::Proposals => s.proposals.is_empty(),
    };
    let (body, side) = if area.width >= 120 && !empty {
        let [a, _, b] = Layout::horizontal([
            Constraint::Percentage(64),
            Constraint::Length(2),
            Constraint::Min(30),
        ])
        .areas(body);
        (a, Some(b))
    } else {
        (body, None)
    };
    let width = body.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut starts: Vec<(usize, usize)> = Vec::new();
    match s.tab {
        Tab::Queue if empty => widgets::empty_state(
            frame,
            body,
            "Nothing queued",
            vec![
                Line::styled(
                    "Press d on an item in Overview, Explorer, Treemap, or Categories.",
                    t.text(),
                ),
                Line::styled(
                    "Queued items are validated again, then moved to the Trash with D.",
                    t.dim(),
                ),
            ],
            t,
        ),
        Tab::Queue => {
            lines.push(Line::styled(
                describe(s.queue.iter().map(|e| (e.size, e.safety))),
                t.text(),
            ));
            let max = s.queue.iter().map(|e| e.size).max().unwrap_or(1);
            for (i, e) in s.queue.iter().enumerate() {
                starts.push((lines.len(), 2));
                lines.extend(entry_lines(e, i == s.cursor, width, 0, max, env));
            }
        }
        Tab::Proposals if empty => widgets::empty_state(
            frame,
            body,
            "No proposals",
            vec![
                Line::styled(
                    "Agents propose cleanups through `spacehog mcp`; they appear here.",
                    t.text(),
                ),
                Line::styled(
                    "Nothing is removed until you approve it here or with `spacehog proposals approve`.",
                    t.dim(),
                ),
            ],
            t,
        ),
        Tab::Proposals => {
            for (i, p) in s.proposals.iter().enumerate() {
                let start = lines.len();
                lines.extend(proposal_lines(p, i == s.cursor, width, env));
                starts.push((start, lines.len() - start));
            }
        }
    }
    if let Some(side) = side {
        render_summary(frame, side, s, env);
    }
    let h = body.height as usize;
    if let Some(&(start, len)) = starts.get(s.cursor) {
        if start < s.scroll {
            s.scroll = start.saturating_sub(1);
        } else if start + len > s.scroll + h {
            s.scroll = (start + len).saturating_sub(h).min(start);
        }
    }
    s.scroll = s.scroll.min(lines.len().saturating_sub(1));
    s.rows.clear();
    for (i, &(start, len)) in starts.iter().enumerate() {
        for line in start..start + len {
            if line >= s.scroll && line < s.scroll + h {
                let y = body.y + (line - s.scroll) as u16;
                s.rows.push((Rect::new(body.x, y, body.width, 1), i));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines).scroll((s.scroll as u16, 0)), body);

    let keys: &[(&str, &str)] = match s.tab {
        Tab::Queue => &[
            ("D", "clean now"),
            ("m", "trash/permanent"),
            ("x", "drop"),
            ("→", "proposals"),
        ],
        Tab::Proposals => &[
            ("enter", "approve"),
            ("x", "reject"),
            ("D", "run now"),
            ("←", "queue"),
        ],
    };
    let mut spans = Vec::new();
    for (k, label) in keys {
        spans.push(Span::styled(*k, t.title()));
        spans.push(Span::styled(format!(" {label}  "), t.dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), hints);

    match &s.phase {
        Phase::Rejecting { id, reason } => {
            let agent = s
                .proposals
                .iter()
                .find(|p| &p.id == id)
                .map_or("the agent", |p| p.agent.as_str());
            let rect = popup(area, 64, 7);
            let block = panel(t, "Reject proposal");
            let inner = block.inner(rect);
            frame.render_widget(Clear, rect);
            frame.render_widget(block, rect);
            let [msg, input, hint] = Layout::vertical([
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(inner);
            frame.render_widget(
                Paragraph::new(format!(
                    "Why reject {agent}'s proposal? The agent reads this."
                ))
                .style(t.text())
                .wrap(Wrap { trim: true }),
                msg,
            );
            input_line(frame, input, t, "› ", reason);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("enter", t.title()),
                    Span::styled(" reject  ", t.dim()),
                    Span::styled("esc", t.title()),
                    Span::styled(" cancel", t.dim()),
                ])),
                hint,
            );
        }
        Phase::Reviewing { entries, at, .. } => {
            let e = &entries[*at];
            let n = entries
                .iter()
                .filter(|e| e.safety == Safety::Review)
                .count();
            let k = entries[..=*at]
                .iter()
                .filter(|e| e.safety == Safety::Review)
                .count();
            let rect = popup(area, 72, 9);
            let block = panel(t, &format!("Review item {k} of {n}"));
            let inner = block.inner(rect);
            frame.render_widget(Clear, rect);
            frame.render_widget(block, rect);
            let w = inner.width as usize;
            let lines = vec![
                Line::styled(truncate_middle(&env.ctx.tilde(&e.path), w), t.title()),
                Line::from(vec![
                    Span::styled(human::bytes(e.size), t.text()),
                    Span::styled(
                        format!("  {} {}", e.safety.glyph(), e.safety.as_str()),
                        Style::new().fg(t.warn()),
                    ),
                ]),
                Line::styled(e.why_safe(env.ctx.config.stale_days), t.dim()),
                Line::default(),
                Line::from(vec![
                    Span::styled("y", t.title()),
                    Span::styled(" clean it  ", t.dim()),
                    Span::styled("n", t.title()),
                    Span::styled(" keep it  ", t.dim()),
                    Span::styled("esc", t.title()),
                    Span::styled(" cancel", t.dim()),
                ]),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        }
        Phase::Confirming {
            ready,
            typed,
            wrong,
        } => {
            let rect = popup(area, 66, 9);
            let block = panel(t, "Delete permanently");
            let inner = block.inner(rect);
            frame.render_widget(Clear, rect);
            frame.render_widget(block, rect);
            let [msg, input, hint] = Layout::vertical([
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(2),
            ])
            .areas(inner);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(
                        format!(
                            "{} cannot be undone.",
                            describe(ready.entries.iter().map(|e| (e.size, e.safety)))
                        ),
                        t.text(),
                    ),
                    Line::from(vec![
                        Span::styled("Type the total to confirm: ", t.text()),
                        Span::styled(ready.total_human.clone(), t.title()),
                    ]),
                ])
                .wrap(Wrap { trim: true }),
                msg,
            );
            input_line(frame, input, t, "› ", typed);
            let note = if *wrong {
                Line::styled(
                    format!("That is not {}. Nothing was deleted.", ready.total_human),
                    Style::new().fg(t.warn()),
                )
            } else {
                Line::from(vec![
                    Span::styled("enter", t.title()),
                    Span::styled(" delete  ", t.dim()),
                    Span::styled("esc", t.title()),
                    Span::styled(" cancel", t.dim()),
                ])
            };
            frame.render_widget(Paragraph::new(note), hint);
        }
        Phase::Refused(rejected) => {
            let rect = popup(area, 80, rejected.len() as u16 + 5);
            let block = panel(t, "Nothing could be cleaned");
            let inner = block.inner(rect);
            frame.render_widget(Clear, rect);
            frame.render_widget(block, rect);
            let mut lines = rejection_lines(rejected, inner.width as usize, env);
            lines.push(Line::default());
            lines.push(Line::styled("esc to go back", t.dim()));
            frame.render_widget(Paragraph::new(lines), inner);
        }
        _ => {}
    }
}

/// The side panel on wide terminals: what `D` would do, split by safety
/// class and bucket, so the list on the left can stay a list.
fn render_summary(frame: &mut Frame, area: Rect, s: &CleanupState, env: &Env) {
    let t = env.theme;
    let ctx = env.ctx;
    let (title, entries, mode, extra): (String, Vec<Entry>, DeleteMode, Vec<Line>) = match s.tab {
        Tab::Queue => (" This cleanup ".into(), s.queue.clone(), s.mode, Vec::new()),
        Tab::Proposals => match s.selected_proposal() {
            Some(p) => (
                format!(" {}'s proposal ", p.agent),
                p.items
                    .iter()
                    .map(|i| Entry::from_proposal(i, &ctx.rules))
                    .collect(),
                p.mode,
                vec![
                    Line::styled(
                        format!("“{}”", p.rationale),
                        t.text().add_modifier(Modifier::ITALIC),
                    ),
                    Line::default(),
                    Line::styled(
                        format!(
                            "{} · created {} · {}",
                            p.id,
                            data::ago(p.created_at),
                            p.status.as_str()
                        ),
                        t.dim(),
                    ),
                    Line::default(),
                ],
            ),
            None => return,
        },
    };
    let block = Block::new()
        .borders(Borders::LEFT | Borders::TOP)
        .border_style(t.border())
        .title(Span::styled(title, t.title()));
    let inner = block.inner(area).inner(ratatui::layout::Margin::new(1, 0));
    frame.render_widget(block, area);
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let extra_h = extra.len() as u16 + 2;
    let [head, figure, stacked, buckets, foot] = Layout::vertical([
        Constraint::Length(extra_h),
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(inner);
    let mut head_lines = extra;
    head_lines.push(Line::styled(
        describe(entries.iter().map(|e| (e.size, e.safety))),
        t.text(),
    ));
    frame.render_widget(Paragraph::new(head_lines).wrap(Wrap { trim: true }), head);
    frame.render_widget(
        Paragraph::new(vec![Line::from(vec![
            Span::styled(
                human::bytes(total),
                Style::new().fg(t.good()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to free", t.dim()),
        ])]),
        figure,
    );
    let mut by_class: Vec<(Safety, u64)> = Vec::new();
    for e in &entries {
        match by_class.iter_mut().find(|(c, _)| *c == e.safety) {
            Some((_, b)) => *b += e.size,
            None => by_class.push((e.safety, e.size)),
        }
    }
    by_class.sort();
    let segs: Vec<widgets::Segment> = by_class
        .into_iter()
        .map(|(c, bytes)| widgets::Segment {
            label: format!("{} {}", c.glyph(), c.as_str()),
            bytes,
            colour: t.safety_rgb(c),
        })
        .collect();
    widgets::stacked(
        frame.buffer_mut(),
        Rect {
            height: 2,
            ..stacked
        },
        &segs,
        t,
    );

    let mut by_bucket: Vec<(Option<String>, u64)> = Vec::new();
    for e in &entries {
        match by_bucket.iter_mut().find(|(b, _)| *b == e.bucket) {
            Some((_, n)) => *n += e.size,
            None => by_bucket.push((e.bucket.clone(), e.size)),
        }
    }
    by_bucket.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let w = buckets.width as usize;
    let label_w = 16.min(w / 3);
    let bar_w = w.saturating_sub(label_w + 11);
    let max = by_bucket.first().map_or(1, |b| b.1).max(1);
    let lines: Vec<Line> = by_bucket
        .iter()
        .take(buckets.height as usize)
        .map(|(b, n)| {
            let label = b
                .as_deref()
                .and_then(|b| ctx.rules.bucket(b))
                .map_or_else(|| "Other".to_string(), |d| d.label.clone());
            let mut spans = vec![Span::styled(
                format!("{:<label_w$} ", truncate_middle(&label, label_w)),
                t.text(),
            )];
            spans.extend(
                widgets::bar(widgets::Bar {
                    frac: *n as f64 / max as f64,
                    width: bar_w,
                    colour: t.bucket_of(&ctx.rules, b.as_deref()),
                    theme: t,
                })
                .spans,
            );
            spans.push(Span::styled(format!(" {:>9}", human::bytes(*n)), t.text()));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), buckets);
    let how = match mode {
        DeleteMode::Trash => Line::from(vec![
            Span::styled("Trash", t.text().add_modifier(Modifier::BOLD)),
            Span::styled(" · recoverable; u undoes the last cleanup", t.dim()),
        ]),
        DeleteMode::Permanent => Line::from(vec![
            Span::styled(
                "Permanent",
                Style::new().fg(t.warn()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · you type the total to confirm", t.dim()),
        ]),
    };
    frame.render_widget(
        Paragraph::new(vec![
            how,
            Line::from(vec![
                Span::styled("D", t.title()),
                Span::styled(" re-checks every item on disk, then cleans", t.dim()),
            ]),
        ])
        .wrap(Wrap { trim: true }),
        foot,
    );
}

fn entry_lines(
    e: &Entry,
    selected: bool,
    width: usize,
    indent: usize,
    max: u64,
    env: &Env,
) -> Vec<Line<'static>> {
    let t = env.theme;
    let ctx = env.ctx;
    let pad = " ".repeat(indent);
    let label = e
        .bucket
        .as_deref()
        .and_then(|b| ctx.rules.bucket(b))
        .map_or_else(|| "Other".to_string(), |b| b.label.clone());
    let bar_w = if width >= 100 { 16 } else { 0 };
    let fixed = indent + 2 + 1 + bar_w + 10 + 16 + 14;
    let path_w = width.saturating_sub(fixed).max(12);
    let marker = if selected { "› " } else { "  " };
    let mut spans = vec![
        Span::styled(format!("{pad}{marker}"), Style::new().fg(t.accent())),
        Span::styled(
            format!("{:<path_w$} ", truncate_middle(&ctx.tilde(&e.path), path_w)),
            t.text(),
        ),
    ];
    if bar_w > 0 {
        spans.extend(
            widgets::bar(widgets::Bar {
                frac: e.size as f64 / max.max(1) as f64,
                width: bar_w,
                colour: t.bucket_of(&ctx.rules, e.bucket.as_deref()),
                theme: t,
            })
            .spans,
        );
    }
    spans.extend([
        Span::styled(format!(" {:>9}", human::bytes(e.size)), t.text()),
        Span::styled(
            format!("  {:<14} ", truncate_middle(&label, 14)),
            Style::new().fg(t.bucket(&ctx.rules, e.bucket.as_deref())),
        ),
        Span::styled(
            format!("{} {}", e.safety.glyph(), e.safety.as_str()),
            safety_style(e.safety, t),
        ),
    ]);
    let first = Line::from(spans);
    let why = format!("{pad}    {}", e.why_safe(ctx.config.stale_days));
    let regen = e
        .regen_hint
        .as_ref()
        .map(|h| format!(" · regen: {h}"))
        .unwrap_or_default();
    let used = why.chars().count();
    let why = if used + 16 <= width {
        format!("{why}{}", truncate_end(&regen, width - used))
    } else {
        truncate_end(&why, width)
    };
    let second = Line::styled(why, t.dim());
    if selected {
        vec![first.style(t.selected()), second.style(t.selected())]
    } else {
        vec![first, second]
    }
}

fn truncate_end(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn safety_style(s: Safety, t: &Theme) -> Style {
    match s {
        Safety::Review => Style::new().fg(t.warn()),
        Safety::Protected => Style::new().fg(t.accent()),
        _ => t.dim(),
    }
}

fn proposal_lines(p: &Proposal, selected: bool, width: usize, env: &Env) -> Vec<Line<'static>> {
    let t = env.theme;
    let status = match p.status {
        ProposalStatus::Approved => Span::styled(" approved", Style::new().fg(t.good())),
        _ => Span::styled(" pending", Style::new().fg(t.warn())),
    };
    let mut head = Line::from(vec![
        Span::styled(
            if selected { "› " } else { "  " },
            Style::new().fg(t.accent()),
        ),
        Span::styled(p.agent.clone(), t.title()),
        Span::styled(format!(" · {} · ", p.total_human), t.text()),
        Span::styled(
            format!(
                "{} item{} · {} · {}",
                p.items.len(),
                if p.items.len() == 1 { "" } else { "s" },
                data::ago(p.created_at),
                p.mode.as_str()
            ),
            t.dim(),
        ),
        status,
        Span::styled(format!("  {}", p.id), t.dim()),
    ]);
    if selected {
        head = head.style(t.selected());
    }
    let mut out = vec![
        head,
        Line::styled(
            truncate_middle(&format!("    “{}”", p.rationale), width),
            t.text().add_modifier(Modifier::ITALIC),
        ),
    ];
    let max = p.items.iter().map(|i| i.size).max().unwrap_or(1);
    for i in &p.items {
        out.extend(entry_lines(
            &Entry::from_proposal(i, &env.ctx.rules),
            false,
            width,
            2,
            max,
            env,
        ));
    }
    out.push(Line::default());
    out
}

fn rejection_lines(rejected: &[Rejection], width: usize, env: &Env) -> Vec<Line<'static>> {
    let t = env.theme;
    rejected
        .iter()
        .map(|r| {
            let path = env.ctx.tilde(&r.path);
            let path_w = (width / 2).max(12);
            Line::from(vec![
                Span::styled("✗ ", Style::new().fg(t.warn())),
                Span::styled(truncate_middle(&path, path_w), t.text()),
                Span::styled(
                    truncate_middle(&format!("  {}", r.reason), width.saturating_sub(path_w + 2)),
                    t.dim(),
                ),
            ])
        })
        .collect()
}

fn render_run(frame: &mut Frame, area: Rect, s: &CleanupState, env: &Env) {
    let t = env.theme;
    let (ready, streamed, counter, done): (&Ready, Vec<Streamed>, &Counter, Option<&Done>) =
        match &s.phase {
            Phase::Running(run) => (&run.ready, run.streamed.clone(), &run.counter, None),
            Phase::Done(d) => {
                let rows = match &d.outcome {
                    Ok(r) => r
                        .results
                        .iter()
                        .map(|x| Streamed {
                            path: x.path.clone(),
                            size: x.size,
                            outcome: format!("{:?}", x.outcome).to_lowercase(),
                            error: x.reason.clone(),
                        })
                        .collect(),
                    Err(_) => Vec::new(),
                };
                (&d.ready, rows, &d.counter, Some(d))
            }
            _ => return,
        };
    let total = ready.entries.len();
    let finished = streamed.len();
    let big_h = if area.height >= 20 { 4 } else { 0 };
    let [head, big, counter_area, gauge, _, list, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(big_h),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(area);
    let verb = match (done.is_some(), ready.mode) {
        (false, DeleteMode::Trash) => "Moving to the Trash",
        (false, DeleteMode::Permanent) => "Deleting permanently",
        (true, _) => "Cleanup finished",
    };
    let whose = match ready.target {
        Target::Queue => format!("· your queue · {}", ready.id),
        Target::Proposal(_) => format!("· {}'s proposal {}", ready.agent, ready.id),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {verb} "), t.title()),
            Span::styled(whose, t.dim()),
        ])),
        head,
    );
    let freed = human::bytes(counter.shown);
    let green = Style::new().fg(t.good()).add_modifier(Modifier::BOLD);
    if big_h > 0 {
        frame.render_widget(
            BigText::builder()
                .pixel_size(PixelSize::Quadrant)
                .style(green)
                .lines(vec![freed.clone().into()])
                .build(),
            big.inner(ratatui::layout::Margin::new(1, 0)),
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {freed} freed"), green),
            Span::styled(format!(" of {}", ready.total_human), t.dim()),
        ])),
        counter_area,
    );
    let ratio = if total == 0 {
        1.0
    } else {
        finished as f64 / total as f64
    };
    frame.render_widget(
        Gauge::default()
            .ratio(ratio.min(1.0))
            .label(format!("{finished}/{total} items"))
            .gauge_style(Style::new().fg(t.accent()))
            .use_unicode(true),
        gauge,
    );

    let width = list.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    if !ready.rejected.is_empty() {
        lines.push(Line::styled(
            format!("Not proposed ({}):", ready.rejected.len()),
            Style::new().fg(t.warn()),
        ));
        lines.extend(rejection_lines(&ready.rejected, width, env));
        lines.push(Line::default());
    }
    if let Some(Done {
        outcome: Err(e), ..
    }) = done
    {
        lines.push(Line::styled(
            format!("The executor refused: {e}"),
            Style::new().fg(t.warn()),
        ));
    }
    for e in &ready.entries {
        let got = streamed.iter().find(|x| x.path == e.path);
        let (glyph, style, note) = match got {
            None => ("…", t.dim(), String::new()),
            Some(x) if x.freed() => ("✓", Style::new().fg(t.good()), x.outcome.clone()),
            Some(x) => (
                "✗",
                Style::new().fg(t.warn()),
                format!(
                    "{}: {}",
                    x.outcome,
                    x.error.clone().unwrap_or_else(|| "no reason given".into())
                ),
            ),
        };
        let path_w = width.saturating_sub(14).min(width / 2 + 10).max(12);
        lines.push(Line::from(vec![
            Span::styled(format!(" {glyph} "), style),
            Span::styled(
                format!(
                    "{:<path_w$}",
                    truncate_middle(&env.ctx.tilde(&e.path), path_w)
                ),
                t.text(),
            ),
            Span::styled(format!(" {:>9}", human::bytes(e.size)), t.text()),
            Span::styled(
                truncate_middle(&format!("  {note}"), width.saturating_sub(path_w + 13)),
                t.dim(),
            ),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(t.border())
                .title(Span::styled(" Items ", t.title())),
        ),
        list,
    );

    let foot_lines = match done {
        None => vec![Line::styled(" working…", t.dim())],
        Some(d) => {
            let mut v = Vec::new();
            match &d.outcome {
                Ok(r) => v.push(Line::from(vec![
                    Span::styled(" journal ", t.dim()),
                    Span::styled(r.journal_id.clone(), t.text()),
                    Span::styled("  ", t.dim()),
                    Span::styled("u", t.title()),
                    Span::styled(
                        if r.mode == DeleteMode::Trash {
                            " undo  "
                        } else {
                            " undo (Trash only)  "
                        },
                        t.dim(),
                    ),
                    Span::styled("esc", t.title()),
                    Span::styled(" back", t.dim()),
                ])),
                Err(_) => v.push(Line::from(vec![
                    Span::styled(" esc", t.title()),
                    Span::styled(" back", t.dim()),
                ])),
            }
            if let Some(msg) = &d.undone {
                v.push(Line::styled(format!(" {msg}"), t.text()));
            }
            v
        }
    };
    frame.render_widget(Paragraph::new(foot_lines), foot);
}

fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn panel<'a>(t: &Theme, title: &str) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.accent()))
        .title(Span::styled(format!(" {title} "), t.title()))
}

fn input_line(frame: &mut Frame, area: Rect, t: &Theme, prompt: &str, input: &Input) {
    let width = area.width.saturating_sub(prompt.len() as u16 + 1) as usize;
    let scroll = input.visual_scroll(width);
    let shown: String = input.value().chars().skip(scroll).collect();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prompt.to_string(), t.title()),
            Span::styled(shown, t.text()),
        ])),
        area,
    );
    frame.set_cursor_position((
        area.x
            + prompt.chars().count() as u16
            + (input.visual_cursor().saturating_sub(scroll)) as u16,
        area.y,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_names_the_safety_mix() {
        assert_eq!(
            describe([
                (1_000_000, Safety::Regenerable),
                (2_000_000, Safety::Regenerable)
            ]),
            "2 items · 3.0 MB · all regenerable"
        );
        assert_eq!(
            describe([
                (1_000, Safety::Cache),
                (1_000, Safety::Regenerable),
                (1_000, Safety::Cache)
            ]),
            "3 items · 3.0 kB · 1 regenerable, 2 cache"
        );
    }
}
