use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use spacehog_core::human;
use spacehog_core::model::SizeMode;
use spacehog_core::report::{self, Explanation};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::TuiOptions;
use crate::data::{Ctx, Snapshot, Toggles};
use crate::explorer::{ExplorerState, RowFilter};
use crate::keys::{self, Action, Key, View};
use crate::overview::OverviewState;
use crate::scanning::ScanTask;
use crate::theme::Theme;

/// Whatever currently owns the keyboard on top of the view.
pub enum Overlay {
    Help,
    Palette(Palette),
    Info(Box<Explanation>),
    Scan,
    Filter,
}

#[derive(Default)]
pub struct Palette {
    pub input: Input,
    pub cursor: usize,
}

impl Palette {
    /// Actions matching the query, best first.
    pub fn matches(&self, view: View) -> Vec<(Key, Action)> {
        let mut scored: Vec<(i32, Key, Action)> = keys::actions(view)
            .into_iter()
            .filter_map(|(k, a)| {
                keys::fuzzy_score(self.input.value(), &a.label()).map(|s| (s, k, a))
            })
            .collect();
        scored.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
        scored.into_iter().map(|(_, k, a)| (k, a)).collect()
    }
}

/// Data for one finished scan and the per-view state built from it.
pub struct Session {
    pub snap: Snapshot,
    pub overview: OverviewState,
    pub explorer: ExplorerState,
}

pub struct App {
    pub ctx: Ctx,
    pub theme: Theme,
    pub view: View,
    pub overlay: Option<Overlay>,
    pub toggles: Toggles,
    pub filter: Input,
    /// None until the index holds a finished scan; the root picker shows instead.
    pub session: Option<Session>,
    pub picker: Input,
    pub scan: Option<ScanTask>,
    /// Paths marked with `d`. M5 turns these into a proposal.
    pub queue: Vec<PathBuf>,
    pub proposals: (u64, u64),
    pub flash: Option<String>,
    pub quit: bool,
    polled: Option<Instant>,
}

pub const PROPOSAL_POLL: Duration = Duration::from_secs(2);
pub const ANIMATION: Duration = Duration::from_millis(100);

impl App {
    pub fn new(ctx: Ctx, theme: Theme, opts: &TuiOptions) -> Result<Self> {
        let picker = Input::new(ctx.config.default_root().to_string_lossy().into_owned());
        let mut app = Self {
            ctx,
            theme,
            view: if opts.changes {
                View::Changes
            } else {
                View::Overview
            },
            overlay: None,
            toggles: Toggles::default(),
            filter: Input::default(),
            session: None,
            picker,
            scan: None,
            queue: Vec::new(),
            proposals: (0, 0),
            flash: None,
            quit: false,
            polled: None,
        };
        app.load_session(None)?;
        app.poll_proposals()?;
        if let Some(root) = &opts.root {
            app.start_scan(root);
        }
        Ok(app)
    }

    fn row_filter(&self) -> RowFilter<'_> {
        RowFilter {
            toggles: self.toggles,
            text: self.filter.value(),
            stale_days: self.ctx.config.stale_days,
        }
    }

    pub fn load_session(&mut self, root: Option<&Path>) -> Result<()> {
        let Some(snap) = Snapshot::latest(&self.ctx.index, root)? else {
            return Ok(());
        };
        let f = self.row_filter();
        let explorer = ExplorerState::new(&self.ctx, snap.root.clone(), f)?;
        let mut overview = OverviewState::default();
        overview.reload(&self.ctx, &snap, self.toggles, self.filter.value())?;
        self.session = Some(Session {
            snap,
            overview,
            explorer,
        });
        Ok(())
    }

    /// Re-runs queries after a toggle or filter change.
    fn refresh(&mut self) -> Result<()> {
        let f = RowFilter {
            toggles: self.toggles,
            text: self.filter.value(),
            stale_days: self.ctx.config.stale_days,
        };
        let Some(s) = self.session.as_mut() else {
            return Ok(());
        };
        s.overview
            .reload(&self.ctx, &s.snap, self.toggles, self.filter.value())?;
        s.explorer.resort(self.toggles);
        s.explorer.clamp(f);
        Ok(())
    }

    pub fn start_scan(&mut self, root: &Path) {
        let root = expand_tilde(root, &self.ctx.home);
        match self.ctx.spawn_scan(&root) {
            Ok(job) => {
                let mut task = ScanTask::new(job);
                task.tick();
                self.scan = Some(task);
                self.overlay = Some(Overlay::Scan);
            }
            Err(e) => self.flash = Some(format!("scan failed to start: {e}")),
        }
    }

    pub fn poll_proposals(&mut self) -> Result<bool> {
        self.polled = Some(Instant::now());
        let next = self.ctx.index.pending_summary()?;
        let changed = next != self.proposals;
        self.proposals = next;
        Ok(changed)
    }

    /// How long the event loop may sleep before `tick` has work to do.
    pub fn next_tick(&self) -> Duration {
        if self.scan.is_some() {
            return ANIMATION;
        }
        let since = self.polled.map_or(PROPOSAL_POLL, |t| t.elapsed());
        PROPOSAL_POLL.saturating_sub(since)
    }

    /// Advances the scan and polls proposals. Returns whether anything visible changed.
    pub fn tick(&mut self) -> Result<bool> {
        let mut changed = false;
        if let Some(task) = self.scan.as_mut() {
            task.tick();
            changed = true;
            if task.is_finished() {
                self.finish_scan()?;
            }
        }
        if self.polled.is_none_or(|t| t.elapsed() >= PROPOSAL_POLL) {
            changed |= self.poll_proposals()?;
        }
        Ok(changed)
    }

    fn finish_scan(&mut self) -> Result<()> {
        let Some(task) = self.scan.take() else {
            return Ok(());
        };
        let root = task.root.clone();
        let elapsed = task.elapsed;
        if matches!(self.overlay, Some(Overlay::Scan)) {
            self.overlay = None;
        }
        match task.join() {
            Ok((record, _)) => {
                self.load_session(Some(&root))?;
                self.flash = Some(format!(
                    "scanned {} in {:.1}s",
                    human::bytes(record.bytes),
                    elapsed.as_secs_f64()
                ));
            }
            Err(e) => self.flash = Some(format!("scan failed: {e}")),
        }
        Ok(())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        self.flash = None;
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return Ok(());
        }
        if let Some(overlay) = self.overlay.take() {
            return self.overlay_key(overlay, key);
        }
        if self.session.is_none() && self.scan.is_none() {
            return self.picker_key(key);
        }
        match keys::lookup(self.view, Key::from_event(key)) {
            Some(action) => self.dispatch(action),
            None => Ok(()),
        }
    }

    fn picker_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => self.quit = true,
            KeyCode::Enter => {
                let root = PathBuf::from(self.picker.value());
                self.start_scan(&root);
            }
            _ => {
                self.picker.handle_event(&Event::Key(key));
            }
        }
        Ok(())
    }

    /// Handles a key for the overlay taken off `self`, putting it back unless it closes.
    fn overlay_key(&mut self, overlay: Overlay, key: KeyEvent) -> Result<()> {
        let keep = match overlay {
            Overlay::Help | Overlay::Info(_) => !matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q' | '?' | 'i') | KeyCode::Enter
            ),
            Overlay::Scan => key.code != KeyCode::Esc,
            Overlay::Filter => match key.code {
                KeyCode::Esc => {
                    self.filter.reset();
                    self.refresh()?;
                    false
                }
                KeyCode::Enter => false,
                _ => {
                    if self.filter.handle_event(&Event::Key(key)).is_some() {
                        self.refresh()?;
                    }
                    true
                }
            },
            Overlay::Palette(mut p) => {
                match key.code {
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Up => p.cursor = p.cursor.saturating_sub(1),
                    KeyCode::Down => p.cursor += 1,
                    KeyCode::Enter => {
                        let chosen = p.matches(self.view).get(p.cursor).map(|(_, a)| *a);
                        return match chosen {
                            Some(a) => self.dispatch(a),
                            None => Ok(()),
                        };
                    }
                    _ => {
                        if p.input.handle_event(&Event::Key(key)).is_some() {
                            p.cursor = 0;
                        }
                    }
                }
                p.cursor = p.cursor.min(p.matches(self.view).len().saturating_sub(1));
                self.overlay = Some(Overlay::Palette(p));
                return Ok(());
            }
        };
        if keep {
            self.overlay = Some(overlay);
        }
        Ok(())
    }

    /// Path the selection-scoped actions (queue, copy, reveal, info) act on.
    fn target(&self) -> Option<PathBuf> {
        let s = self.session.as_ref()?;
        match self.view {
            View::Overview => s.overview.selected_item().map(|i| PathBuf::from(&i.path)),
            View::Explorer => s.explorer.selected().map(|n| PathBuf::from(&n.path)),
            _ => None,
        }
    }

    pub fn dispatch(&mut self, action: Action) -> Result<()> {
        let toggles = self.toggles;
        let text = self.filter.value().to_string();
        let f = RowFilter {
            toggles,
            text: &text,
            stale_days: self.ctx.config.stale_days,
        };
        match action {
            Action::Quit if self.view != View::Overview => self.view = View::Overview,
            Action::Quit => self.quit = true,
            Action::Back if !self.filter.value().is_empty() => {
                self.filter.reset();
                self.refresh()?;
            }
            Action::Back => self.view = View::Overview,
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::Palette => self.overlay = Some(Overlay::Palette(Palette::default())),
            Action::Filter => self.overlay = Some(Overlay::Filter),
            Action::Rescan => match self.scan {
                Some(_) => self.overlay = Some(Overlay::Scan),
                None => {
                    let root = self
                        .session
                        .as_ref()
                        .map(|s| PathBuf::from(&s.snap.root.path))
                        .unwrap_or_else(|| PathBuf::from(self.picker.value()));
                    self.start_scan(&root);
                }
            },
            Action::ToggleSize => {
                self.toggles.size = match self.toggles.size {
                    SizeMode::Disk => SizeMode::Apparent,
                    SizeMode::Apparent => SizeMode::Disk,
                };
                self.refresh()?;
            }
            Action::ToggleStale => {
                self.toggles.stale_only = !self.toggles.stale_only;
                self.refresh()?;
            }
            Action::ToggleHidden => {
                self.toggles.hidden = !self.toggles.hidden;
                self.refresh()?;
            }
            Action::NextView => self.view = self.view.next_built(),
            Action::Show(v) => self.view = v,
            Action::Queue => {
                if let Some(p) = self.target() {
                    if !self.queue.contains(&p) {
                        self.queue.push(p);
                    }
                    self.flash = Some(format!("{} queued for cleanup", self.queue.len()));
                }
            }
            Action::Reveal => {
                if let Some(p) = self.target() {
                    self.flash = Some(match crate::sys::reveal(&p) {
                        Ok(()) => format!("opened {}", self.ctx.tilde(&p.to_string_lossy())),
                        Err(e) => format!("could not open: {e}"),
                    });
                }
            }
            Action::CopyPath => {
                if let Some(p) = self.target() {
                    self.flash = Some(match crate::sys::copy(&p.to_string_lossy()) {
                        Ok(()) => "copied path".into(),
                        Err(e) => format!("could not copy: {e}"),
                    });
                }
            }
            Action::Info => {
                if let Some(p) = self.target() {
                    match report::explain(
                        &self.ctx.index,
                        &self.ctx.rules,
                        &p,
                        self.ctx.config.stale_days,
                    ) {
                        Ok(e) => self.overlay = Some(Overlay::Info(Box::new(e))),
                        Err(e) => self.flash = Some(format!("no explanation: {e}")),
                    }
                }
            }
            _ => {
                let Some(s) = self.session.as_mut() else {
                    return Ok(());
                };
                match (self.view, action) {
                    (View::Overview, Action::Up) => s.overview.step(-1),
                    (View::Overview, Action::Down) => s.overview.step(1),
                    (View::Overview, Action::Top) => s.overview.jump(false),
                    (View::Overview, Action::Bottom) => s.overview.jump(true),
                    (View::Overview, Action::PrevBucket | Action::NextBucket) => {
                        s.overview
                            .cycle_bucket(if action == Action::NextBucket { 1 } else { -1 });
                        s.overview
                            .reload_top(&self.ctx, &s.snap, toggles, self.filter.value())?;
                    }
                    (View::Overview, Action::Open) => {
                        if let Some(id) = s.overview.selected_item().map(|i| i.node_id)
                            && s.explorer.reveal(&self.ctx, id, toggles)?
                        {
                            self.view = View::Explorer;
                        }
                    }
                    (View::Explorer, Action::Up) => s.explorer.step(f, -1),
                    (View::Explorer, Action::Down) => s.explorer.step(f, 1),
                    (View::Explorer, Action::Top) => s.explorer.first(f),
                    (View::Explorer, Action::Bottom) => s.explorer.last(f),
                    (View::Explorer, Action::Descend) => {
                        s.explorer.descend(&self.ctx, f)?;
                    }
                    (View::Explorer, Action::Ascend) => {
                        s.explorer.ascend();
                    }
                    (View::Explorer, Action::Preview) => {
                        s.explorer.preview = !s.explorer.preview;
                    }
                    _ => {}
                }
                if self.view == View::Explorer {
                    s.explorer.sync_preview(&self.ctx, toggles)?;
                }
            }
        }
        Ok(())
    }
}

fn expand_tilde(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}
