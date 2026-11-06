use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use spacehog_core::human;
use spacehog_core::model::SizeMode;
use spacehog_core::proposals::{Actor, refusal};
use spacehog_core::report::{self, Explanation};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::TuiOptions;
use crate::categories::CategoriesState;
use crate::changes::ChangesState;
use crate::cleanup::CleanupState;
use crate::data::{Ctx, Snapshot, Toggles};
use crate::explorer::{ExplorerState, RowFilter};
use crate::keys::{self, Action, Key, View};
use crate::overview::OverviewState;
use crate::scanning::ScanTask;
use crate::theme::Theme;
use crate::treemap::TreemapState;

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

/// A clickable region from the last draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    View(View),
    /// A legend row or donut segment; None is "all buckets".
    Bucket(Option<usize>),
    /// The Overview donut; the arc under the cell picks the bucket.
    Donut,
    /// A row in the current view's list.
    Row(usize),
}

/// Data for one finished scan and the per-view state built from it.
pub struct Session {
    pub snap: Snapshot,
    pub overview: OverviewState,
    pub explorer: ExplorerState,
    pub treemap: TreemapState,
    pub categories: CategoriesState,
    pub changes: ChangesState,
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
    /// Paths marked with `d`, cleaned from the Cleanup view's queue tab.
    pub queue: Vec<PathBuf>,
    pub cleanup: CleanupState,
    pub proposals: (u64, u64),
    pub flash: Option<String>,
    pub quit: bool,
    pub hits: Vec<(Rect, Hit)>,
    polled: Option<Instant>,
}

pub const PROPOSAL_POLL: Duration = Duration::from_secs(2);
/// Agents' proposals should appear promptly while someone is looking for them.
pub const CLEANUP_POLL: Duration = Duration::from_millis(500);
pub const ANIMATION: Duration = Duration::from_millis(100);
pub const FRAME: Duration = Duration::from_millis(16);

impl App {
    pub fn new(ctx: Ctx, theme: Theme, opts: &TuiOptions) -> Result<Self> {
        let picker = Input::new(ctx.config.default_root().to_string_lossy().into_owned());
        let cleanup = CleanupState::new(ctx.config.cleanup.mode);
        let mut app = Self {
            ctx,
            theme,
            view: View::Overview,
            overlay: None,
            toggles: Toggles::default(),
            filter: Input::default(),
            session: None,
            picker,
            scan: None,
            queue: Vec::new(),
            cleanup,
            proposals: (0, 0),
            flash: None,
            quit: false,
            hits: Vec::new(),
            polled: None,
        };
        app.load_session(None)?;
        app.poll_proposals()?;
        if opts.changes {
            app.show(View::Changes)?;
        }
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
        let mut categories = CategoriesState::default();
        categories.load(&self.ctx, &snap, self.toggles)?;
        self.session = Some(Session {
            treemap: TreemapState::new(snap.root.clone()),
            changes: ChangesState::default(),
            snap,
            overview,
            explorer,
            categories,
        });
        if self.view == View::Changes {
            self.ensure_changes()?;
        }
        Ok(())
    }

    fn ensure_changes(&mut self) -> Result<()> {
        if let Some(s) = self.session.as_mut() {
            let root = s.snap.root.path.clone();
            s.changes.ensure(&self.ctx, &root, s.snap.scan.id)?;
        }
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
        s.treemap.invalidate();
        s.categories.load(&self.ctx, &s.snap, self.toggles)?;
        Ok(())
    }

    /// Switches view, loading whatever the new view reads.
    pub fn show(&mut self, v: View) -> Result<()> {
        self.view = v;
        match v {
            View::Cleanup => {
                self.cleanup.load_queue(&self.ctx, &self.queue)?;
                self.cleanup.load_proposals(&self.ctx)?;
            }
            View::Changes => self.ensure_changes()?,
            _ => {}
        }
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
        if self.view == View::Cleanup && !self.cleanup.captures_keys() {
            self.cleanup.load_proposals(&self.ctx)?;
            return Ok(true);
        }
        Ok(changed)
    }

    fn poll_interval(&self) -> Duration {
        if self.view == View::Cleanup {
            CLEANUP_POLL
        } else {
            PROPOSAL_POLL
        }
    }

    /// Whether something on screen moves on its own and needs frequent ticks.
    pub fn animating(&self) -> bool {
        self.scan.is_some()
            || self.cleanup.animating()
            || self.session.as_ref().is_some_and(|s| s.treemap.animating())
    }

    /// How long the event loop may sleep before `tick` has work to do.
    pub fn next_tick(&self) -> Duration {
        if self.cleanup.animating() || self.session.as_ref().is_some_and(|s| s.treemap.animating())
        {
            return FRAME;
        }
        if self.scan.is_some() {
            return ANIMATION;
        }
        let interval = self.poll_interval();
        let since = self.polled.map_or(interval, |t| t.elapsed());
        interval.saturating_sub(since)
    }

    /// Advances animations, the scan, and a running cleanup, and polls
    /// proposals. Returns whether anything visible changed.
    pub fn tick(&mut self) -> Result<bool> {
        let mut changed = false;
        if let Some(task) = self.scan.as_mut() {
            task.tick();
            changed = true;
            if task.is_finished() {
                self.finish_scan()?;
            }
        }
        if let Some(s) = self.session.as_mut()
            && s.treemap.animating()
        {
            s.treemap.tick();
            changed = true;
        }
        let (redraw, finished) = self.cleanup.tick(&self.ctx, &mut self.queue)?;
        changed |= redraw;
        if finished {
            let root = self
                .session
                .as_ref()
                .map(|s| PathBuf::from(&s.snap.root.path));
            self.reload_after_cleanup(root.as_deref())?;
        }
        if self
            .polled
            .is_none_or(|t| t.elapsed() >= self.poll_interval())
        {
            changed |= self.poll_proposals()?;
        }
        Ok(changed)
    }

    /// The executor marked nodes deleted, so every view's numbers moved.
    fn reload_after_cleanup(&mut self, root: Option<&Path>) -> Result<()> {
        let view = self.view;
        self.load_session(root)?;
        self.view = view;
        self.poll_proposals()?;
        Ok(())
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
        if self.view == View::Cleanup && self.cleanup.captures_keys() {
            self.flash = self.cleanup.key(key, &self.ctx, &self.queue)?;
            return Ok(());
        }
        match keys::lookup(self.view, Key::from_event(key)) {
            Some(action) => self.dispatch(action),
            None => Ok(()),
        }
    }

    pub fn handle_mouse(&mut self, ev: MouseEvent) -> Result<()> {
        if self.overlay.is_some() || self.session.is_none() {
            return Ok(());
        }
        if self.view == View::Cleanup && self.cleanup.captures_keys() {
            return Ok(());
        }
        let pos = Position::new(ev.column, ev.row);
        match ev.kind {
            MouseEventKind::ScrollUp => self.dispatch(Action::Up),
            MouseEventKind::ScrollDown => self.dispatch(Action::Down),
            MouseEventKind::Moved => {
                if self.view == View::Treemap
                    && let Some(s) = self.session.as_mut()
                {
                    s.treemap.hover = s.treemap.hit(ev.column, ev.row);
                }
                Ok(())
            }
            MouseEventKind::Down(MouseButton::Left) => self.click(pos),
            _ => Ok(()),
        }
    }

    fn click(&mut self, pos: Position) -> Result<()> {
        let found = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(pos))
            .copied();
        let hit = found.map(|(_, h)| h);
        if let Some(Hit::View(v)) = hit {
            return self.show(v);
        }
        let Some(s) = self.session.as_mut() else {
            return Ok(());
        };
        let toggles = self.toggles;
        match self.view {
            View::Overview => match hit {
                Some(Hit::Donut) => {
                    let area = found.map(|(r, _)| r).unwrap_or_default();
                    if let Some(b) = s.overview.bucket_at(area, pos.x, pos.y) {
                        s.overview.bucket = if s.overview.bucket == Some(b) {
                            None
                        } else {
                            Some(b)
                        };
                        s.overview.cursor = 0;
                        s.overview
                            .reload_top(&self.ctx, &s.snap, toggles, self.filter.value())?;
                    }
                }
                Some(Hit::Bucket(b)) => {
                    s.overview.bucket = if s.overview.bucket == b { None } else { b };
                    s.overview.cursor = 0;
                    s.overview
                        .reload_top(&self.ctx, &s.snap, toggles, self.filter.value())?;
                }
                Some(Hit::Row(i)) if i == s.overview.cursor => return self.dispatch(Action::Open),
                Some(Hit::Row(i)) => s.overview.cursor = i,
                _ => {}
            },
            View::Explorer => {
                let path = s.explorer.tree.rendered_at(pos).map(<[_]>::to_vec);
                if let Some(path) = path {
                    let again = path == s.explorer.tree.selected();
                    s.explorer.tree.select(path);
                    if again {
                        return self.dispatch(Action::Descend);
                    }
                    s.explorer.sync_preview(&self.ctx, toggles)?;
                }
            }
            View::Treemap => {
                if let Some(i) = s.treemap.hit(pos.x, pos.y) {
                    s.treemap.cursor = i;
                    return self.dispatch(Action::ZoomIn);
                }
            }
            View::Categories => {
                if let Some(&(_, i)) = s
                    .categories
                    .rows_on_screen
                    .iter()
                    .find(|(r, _)| r.contains(pos))
                {
                    let again = i == s.categories.cursor;
                    s.categories.set_cursor(i);
                    if again {
                        return self.dispatch(Action::Expand);
                    }
                }
            }
            View::Cleanup => {
                if let Some(&(_, i)) = self.cleanup.rows.iter().find(|(r, _)| r.contains(pos)) {
                    self.cleanup.set_cursor(i);
                }
            }
            View::Changes => {}
        }
        Ok(())
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
            View::Treemap => s.treemap.selected().map(|n| PathBuf::from(&n.path)),
            View::Categories => s.categories.current_item().map(|i| PathBuf::from(&i.path)),
            View::Cleanup => self.cleanup.selected_path(),
            View::Changes => None,
        }
    }

    /// Queues paths the executor would accept and names the ones it would
    /// refuse, using the same check `propose` runs.
    pub fn enqueue(&mut self, paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
        let mut added = Vec::new();
        let mut refused: Vec<String> = Vec::new();
        for p in paths {
            match refusal(
                &self.ctx.index,
                &self.ctx.rules,
                &self.ctx.config,
                Actor::User,
                &p,
            )? {
                Err(reason) => {
                    refused.push(format!("{} {reason}", self.ctx.tilde(&p.to_string_lossy())))
                }
                Ok(_) => {
                    if !self.queue.contains(&p) {
                        self.queue.push(p.clone());
                    }
                    added.push(p);
                }
            }
        }
        self.flash = Some(match (added.len(), refused.as_slice()) {
            (_, []) => format!("{} queued for cleanup", self.queue.len()),
            (0, [one]) => format!("not queued: {one}"),
            (0, many) => format!("not queued: {} refused, e.g. {}", many.len(), many[0]),
            (n, many) => format!("{n} queued; {} refused, e.g. {}", many.len(), many[0]),
        });
        Ok(added)
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
            Action::NextView => self.show(self.view.next())?,
            Action::Show(v) => self.show(v)?,
            Action::Queue if self.view == View::Categories => {
                let Some(s) = self.session.as_ref() else {
                    return Ok(());
                };
                let picked: Vec<PathBuf> = if s.categories.selected.is_empty() {
                    s.categories
                        .current_item()
                        .map(|i| PathBuf::from(&i.path))
                        .into_iter()
                        .collect()
                } else {
                    s.categories.selected.keys().map(PathBuf::from).collect()
                };
                let added = self.enqueue(picked)?;
                if let Some(s) = self.session.as_mut() {
                    for p in added {
                        s.categories.selected.remove(&*p.to_string_lossy());
                    }
                }
            }
            Action::Queue => {
                if let Some(p) = self.target() {
                    self.enqueue(vec![p])?;
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
            _ if self.view == View::Cleanup => {
                if let Some(msg) = self.cleanup.act(action, &self.ctx, &mut self.queue)? {
                    self.flash = Some(msg);
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
                    (View::Treemap, Action::Up) => s.treemap.step(0, -1),
                    (View::Treemap, Action::Down) => s.treemap.step(0, 1),
                    (View::Treemap, Action::Left) => s.treemap.step(-1, 0),
                    (View::Treemap, Action::Right) => s.treemap.step(1, 0),
                    (View::Treemap, Action::ZoomIn) => {
                        s.treemap.zoom_in(&self.ctx, toggles);
                    }
                    (View::Treemap, Action::ZoomOut) => {
                        s.treemap.zoom_out();
                    }
                    (View::Treemap, Action::DeeperMap) => s.treemap.set_depth(1),
                    (View::Treemap, Action::ShallowerMap) => s.treemap.set_depth(-1),
                    (View::Categories, Action::Up) => s.categories.step(-1),
                    (View::Categories, Action::Down) => s.categories.step(1),
                    (View::Categories, Action::Top) => s.categories.jump(false),
                    (View::Categories, Action::Bottom) => s.categories.jump(true),
                    (View::Categories, Action::Expand) => {
                        s.categories.toggle_open(&self.ctx, &s.snap)?;
                    }
                    (View::Categories, Action::Collapse) => s.categories.collapse(),
                    (View::Categories, Action::Select) => {
                        s.categories.select(&self.ctx, &s.snap)?;
                    }
                    (View::Categories, Action::SelectAll) => {
                        s.categories.select_all(&self.ctx, &s.snap)?;
                    }
                    (View::Changes, Action::Up) => s.changes.step(-1),
                    (View::Changes, Action::Down) => s.changes.step(1),
                    (View::Changes, Action::OlderBaseline | Action::NewerBaseline) => {
                        let root = s.snap.root.path.clone();
                        let delta = if action == Action::OlderBaseline {
                            1
                        } else {
                            -1
                        };
                        if !s.changes.shift(&self.ctx, &root, delta)? {
                            self.flash = Some(match delta {
                                1 => "no older scan is kept".into(),
                                _ => "already comparing with the previous scan".into(),
                            });
                        }
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
