use std::f64::consts::TAU;

use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use spacehog_core::human;
use spacehog_core::model::SizeMode;
use spacehog_core::report::Item;
use spacehog_core::rules::Safety;
use tui_big_text::{BigText, PixelSize};

use crate::app::Hit;
use crate::data::{self, BucketSlice, Ctx, SafetySplit, Snapshot, Toggles};
use crate::theme::{self, Rgb, Theme};
use crate::widgets::{self, Bar, Segment};

/// The Overview's data and cursor. `bucket` is the selected donut segment,
/// which narrows the item list to that bucket and lifts its slice out of
/// the donut with a ring of its largest items around it.
#[derive(Debug, Default)]
pub struct OverviewState {
    pub buckets: Vec<BucketSlice>,
    pub top: Vec<Item>,
    pub safety: SafetySplit,
    pub bucket: Option<usize>,
    pub cursor: usize,
    scroll: usize,
}

impl OverviewState {
    pub fn reload(&mut self, ctx: &Ctx, snap: &Snapshot, t: Toggles, filter: &str) -> Result<()> {
        let selected_id = self.selected_bucket().map(|b| b.id.clone());
        self.buckets = data::buckets(ctx, snap, t)?;
        self.bucket = selected_id.and_then(|id| self.buckets.iter().position(|b| b.id == id));
        self.safety = data::safety_split(ctx, snap, t)?;
        self.reload_top(ctx, snap, t, filter)
    }

    pub fn reload_top(
        &mut self,
        ctx: &Ctx,
        snap: &Snapshot,
        t: Toggles,
        filter: &str,
    ) -> Result<()> {
        let bucket = self.selected_bucket().map(|b| b.id.clone());
        self.top = data::top(ctx, snap, t, bucket.as_deref(), filter)?;
        self.cursor = self.cursor.min(self.top.len().saturating_sub(1));
        Ok(())
    }

    pub fn selected_bucket(&self) -> Option<&BucketSlice> {
        self.bucket.and_then(|i| self.buckets.get(i))
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.top.get(self.cursor)
    }

    pub fn step(&mut self, delta: isize) {
        let last = self.top.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    pub fn jump(&mut self, last: bool) {
        self.cursor = if last {
            self.top.len().saturating_sub(1)
        } else {
            0
        };
    }

    /// Steps through segments with "all buckets" as the slot before the first.
    pub fn cycle_bucket(&mut self, delta: isize) {
        let n = self.buckets.len() as isize;
        if n == 0 {
            return;
        }
        let cur = self.bucket.map_or(-1, |i| i as isize);
        let next = (cur + 1 + delta).rem_euclid(n + 1) - 1;
        self.bucket = (next >= 0).then_some(next as usize);
        self.cursor = 0;
    }

    /// Each bucket's arc as (start, end) turns, clockwise from 12 o'clock.
    fn arcs(&self) -> Vec<(f64, f64)> {
        let total: u64 = self.buckets.iter().map(|b| b.bytes).sum();
        let mut acc = 0u64;
        self.buckets
            .iter()
            .map(|b| {
                let from = acc;
                acc += b.bytes;
                if total == 0 {
                    (0.0, 1.0)
                } else {
                    (from as f64 / total as f64, acc as f64 / total as f64)
                }
            })
            .collect()
    }

    /// The selected bucket's largest listed items as arcs inside its slice:
    /// (start, end, index into `top`). Stops before an item would overrun
    /// the bucket or shrink to a sliver; the rest of the slice is "smaller".
    fn item_arcs(&self, mode: SizeMode) -> Vec<(f64, f64, usize)> {
        let Some(i) = self.bucket else {
            return Vec::new();
        };
        let Some(&(from, to)) = self.arcs().get(i) else {
            return Vec::new();
        };
        let bytes = self.buckets[i].bytes.max(1) as f64;
        let (mut at, mut used, mut out) = (from, 0.0, Vec::new());
        for (k, item) in self.top.iter().enumerate().take(8) {
            let b = data::item_size(item, mode) as f64;
            if used + b > bytes || b / bytes < 0.02 {
                break;
            }
            let end = at + b / bytes * (to - from);
            out.push((at, end, k));
            (at, used) = (end, used + b);
        }
        out
    }

    /// The bucket whose arc is under a cell of the donut drawn in `area`.
    pub fn bucket_at(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        let ring = Ring::new(area);
        let (x, y) = cell_centre(area, col, row)?;
        let (r, turn) = ring.polar(x, y, (0.0, 0.0));
        if r > ring.outer + 1.0 {
            return None;
        }
        self.arcs().iter().position(|(_, end)| turn <= *end)
    }

    /// The listed item whose arc in the selected bucket's ring is under a cell.
    pub fn item_at(&self, area: Rect, col: u16, row: u16, mode: SizeMode) -> Option<usize> {
        let ring = Ring::new(area);
        let (x, y) = cell_centre(area, col, row)?;
        let arc = *self.arcs().get(self.bucket?)?;
        let (r, turn) = ring.polar(x, y, ring.lift(arc));
        if r < ring.items_in - 0.5 || r > ring.items_out + 1.0 {
            return None;
        }
        self.item_arcs(mode)
            .into_iter()
            .find(|(from, to, _)| (*from..*to).contains(&turn))
            .map(|(_, _, k)| k)
    }
}

fn cell_centre(area: Rect, col: u16, row: u16) -> Option<(f64, f64)> {
    let x = f64::from(col.checked_sub(area.x)?) + 0.5;
    let y = f64::from(row.checked_sub(area.y)?) * 2.0 + 1.0;
    Some((x, y))
}

/// Donut geometry in half-block pixels, which are about square. The donut
/// leaves room outside it for a selected slice to lift out and ring its items,
/// so selecting never resizes it.
struct Ring {
    cx: f64,
    cy: f64,
    inner: f64,
    outer: f64,
    items_in: f64,
    items_out: f64,
    room: f64,
}

/// Arc length, in pixels, left dark between neighbouring arcs.
const GAP: f64 = 0.8;

impl Ring {
    fn new(area: Rect) -> Self {
        let (w, h) = (f64::from(area.width), f64::from(area.height) * 2.0);
        let room = (w.min(h) / 2.0 - 0.5).max(1.0);
        let outer = room * 0.78;
        Self {
            cx: w / 2.0,
            cy: h / 2.0,
            inner: outer * 0.6,
            outer,
            items_in: room * 0.83,
            items_out: room * 0.97,
            room,
        }
    }

    /// How far a selected arc's centre moves out along its middle.
    fn lift(&self, (from, to): (f64, f64)) -> (f64, f64) {
        let mid = (from + to) / 2.0 * TAU;
        let d = self.room * 0.03;
        (mid.sin() * d, -mid.cos() * d)
    }

    /// Radius and turn (0..1, clockwise from 12 o'clock) of a point, measured
    /// from the centre moved by `shift`.
    fn polar(&self, x: f64, y: f64, shift: (f64, f64)) -> (f64, f64) {
        let (dx, dy) = (x - self.cx - shift.0, y - self.cy - shift.1);
        let r = (dx * dx + dy * dy).sqrt();
        (r, (dx.atan2(-dy) / TAU).rem_euclid(1.0))
    }
}

/// Whether a point at radius `r` and `turn` is clear of the gap at either end
/// of an arc. A lone full circle has no ends.
fn clear_of_ends(turn: f64, (from, to): (f64, f64), r: f64) -> bool {
    to - from >= 1.0 || (turn - from).min(to - turn) * TAU * r >= GAP / 2.0
}

pub struct Env<'a> {
    pub ctx: &'a Ctx,
    pub theme: &'a Theme,
    pub snap: &'a Snapshot,
    pub toggles: Toggles,
    pub queue: &'a [std::path::PathBuf],
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    s: &mut OverviewState,
    f: &Env,
    hits: &mut Vec<(Rect, Hit)>,
) {
    if area.width < 80 {
        let [head, bar, list] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(total_line(s, f)), head);
        render_safety_bar(frame, bar, s, f);
        render_top(frame, list, s, f, hits);
        return;
    }
    let tall = area.height >= 30;
    let [hero, _, safety, _, main] = Layout::vertical([
        Constraint::Length(if tall { 5 } else { 2 }),
        Constraint::Length(1),
        Constraint::Length(if tall { 3 } else { 2 }),
        Constraint::Length(1),
        Constraint::Min(8),
    ])
    .areas(area);
    render_hero(frame, hero, s, f, tall);
    render_safety_bar(frame, safety, s, f);
    let left_w = match main.width {
        w if w >= 120 => w / 2,
        w if w >= 100 => 38,
        _ => 30,
    };
    let [left, _, right] = Layout::horizontal([
        Constraint::Length(left_w),
        Constraint::Length(3),
        Constraint::Min(30),
    ])
    .areas(main);
    render_buckets(frame, left, s, f, hits);
    render_top(frame, right, s, f, hits);
}

fn total_line<'a>(s: &OverviewState, f: &Env<'a>) -> Line<'a> {
    let t = f.theme;
    let mut spans = vec![
        Span::styled(
            human::bytes(s.safety.safe()),
            Style::new().fg(t.good()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" safe to free  · ", t.dim()),
        Span::styled(human::bytes(f.snap.root.size(f.toggles.size)), t.text()),
        Span::styled(" in ", t.dim()),
        Span::styled(f.ctx.tilde(&f.snap.root.path), t.text()),
    ];
    if let Some(b) = s.selected_bucket() {
        spans.push(Span::styled(
            format!("  · {}", b.label),
            Style::new().fg(t.bucket_hex(&b.colour)),
        ));
    }
    Line::from(spans)
}

/// What you can free, as large as the space allows, beside what was scanned
/// and how full the volume is.
fn render_hero(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env, tall: bool) {
    let t = f.theme;
    let safe = human::bytes(s.safety.safe());
    let safe_style = Style::new().fg(t.good()).add_modifier(Modifier::BOLD);
    let caption = Line::from(vec![
        Span::styled("safe to free", t.text()),
        Span::styled(" · ", t.dim()),
        Span::styled(human::bytes(s.safety.stale_safe), Style::new().fg(t.warn())),
        Span::styled(" of it stale", t.dim()),
    ]);
    let side_w = 50.min(area.width / 2);
    let [left, _, side] = Layout::horizontal([
        Constraint::Min(20),
        Constraint::Length(2),
        Constraint::Length(side_w),
    ])
    .areas(area);
    if tall {
        let [num, cap] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(left);
        frame.render_widget(
            BigText::builder()
                .pixel_size(PixelSize::Quadrant)
                .style(safe_style)
                .lines(vec![safe.into()])
                .build(),
            num,
        );
        frame.render_widget(Paragraph::new(caption), cap);
    } else {
        let mut first = vec![Span::styled(safe, safe_style), Span::raw(" ")];
        first.extend(caption.spans);
        frame.render_widget(Paragraph::new(Line::from(first)), left);
    }

    let size_word = match f.toggles.size {
        SizeMode::Disk => " on disk in ",
        SizeMode::Apparent => " apparent size in ",
    };
    let scanned = Line::from(vec![
        Span::styled(
            human::bytes(f.snap.root.size(f.toggles.size)),
            t.text().add_modifier(Modifier::BOLD),
        ),
        Span::styled(size_word, t.dim()),
        Span::styled(f.ctx.tilde(&f.snap.root.path), t.text()),
    ]);
    let files = Line::styled(
        format!(
            "{} files · scanned {}",
            crate::scanning::count(f.snap.scan.files),
            f.snap.scan.finished_at.map_or("never".into(), data::ago)
        ),
        t.dim(),
    );
    let (gauge, free) = match &f.snap.volume {
        Some(v) => {
            let ratio = if v.total == 0 {
                0.0
            } else {
                v.used as f64 / v.total as f64
            };
            let colour = if ratio > 0.9 {
                t.warn_rgb()
            } else {
                t.data_rgb()
            };
            let label = format!("{} ", v.mount);
            let pct = format!(" {:.0}%", ratio * 100.0);
            let w =
                usize::from(side.width).saturating_sub(label.chars().count() + pct.chars().count());
            let mut spans = vec![Span::styled(label, t.dim())];
            spans.extend(
                widgets::bar(Bar {
                    frac: ratio,
                    width: w,
                    colour,
                    theme: t,
                })
                .spans,
            );
            spans.push(Span::styled(
                pct,
                Style::new().fg(t.rgb(colour)).add_modifier(Modifier::BOLD),
            ));
            let free = Line::from(vec![
                Span::styled(
                    v.free_human.clone(),
                    Style::new().fg(t.rgb(colour)).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" free of {}", v.total_human), t.dim()),
            ]);
            (Line::from(spans), free)
        }
        None => (Line::styled("volume unknown", t.dim()), Line::default()),
    };
    let lines = if tall {
        vec![Line::default(), scanned, files, gauge, free]
    } else {
        vec![scanned, gauge]
    };
    frame.render_widget(Paragraph::new(lines), side);
}

/// The safety classes as one bar with their labels under it and, given a
/// third row, a bracket under the classes that make up "safe to free".
fn render_safety_bar(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env) {
    let t = f.theme;
    let sp = s.safety;
    let segs: Vec<Segment> = [
        (Safety::Regenerable, sp.regenerable),
        (Safety::Cache, sp.cache),
        (Safety::Review, sp.review),
        (Safety::Protected, sp.protected),
    ]
    .into_iter()
    .filter(|(_, b)| *b > 0)
    .map(|(class, bytes)| Segment {
        label: format!(
            "{} {} {}",
            class.glyph(),
            class.as_str(),
            human::bytes(bytes)
        ),
        bytes,
        colour: t.safety_rgb(class),
    })
    .collect();
    widgets::stacked(frame.buffer_mut(), area, &segs, t);

    let total = sp.regenerable + sp.cache + sp.review + sp.protected;
    if area.height < 3 || total == 0 || sp.safe() == 0 {
        return;
    }
    let w = ((sp.safe() as f64 / total as f64) * f64::from(area.width)).round() as usize;
    if w < 3 {
        return;
    }
    let label = format!(" {} safe to free ", human::bytes(sp.safe()));
    let n = label.chars().count();
    let line = if w >= n + 4 {
        let left = (w - 2 - n) / 2;
        format!(
            "╰{}{label}{}╯",
            "─".repeat(left),
            "─".repeat(w - 2 - n - left)
        )
    } else {
        format!("╰{}╯", "─".repeat(w - 2))
    };
    let row = Rect::new(area.x, area.y + 2, area.width, 1);
    frame.render_widget(
        Paragraph::new(Span::styled(line, Style::new().fg(t.good()))),
        row,
    );
}

fn render_buckets(
    frame: &mut Frame,
    area: Rect,
    s: &OverviewState,
    f: &Env,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let t = f.theme;
    let [head, _, inner] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Where it goes", t.text().add_modifier(Modifier::BOLD)),
            Span::styled(format!("   {} buckets", s.buckets.len()), t.dim()),
        ])),
        head,
    );
    // The donut goes beside the legend or above it, whichever lets it grow.
    let beside = inner.width.saturating_sub(38).min(inner.height * 2);
    let legend_rows = (s.buckets.len() as u16).min(inner.height / 2);
    let above = inner
        .width
        .min(inner.height.saturating_sub(legend_rows + 1) * 2);
    let (donut, legend) = if inner.width >= 64 && beside >= above {
        let [a, _, b] = Layout::horizontal([
            Constraint::Length(beside),
            Constraint::Length(2),
            Constraint::Min(10),
        ])
        .areas(inner);
        (
            Rect {
                height: beside / 2,
                ..a
            },
            b,
        )
    } else {
        let [a, _, b] = Layout::vertical([
            Constraint::Length((above / 2).max(4)),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(inner);
        (a, b)
    };
    let side = donut.width.min(donut.height * 2);
    let donut = Rect {
        x: donut.x + (donut.width - side) / 2,
        width: side,
        ..donut
    };
    render_donut(frame, donut, s, f);
    hits.push((donut, Hit::Donut));
    render_legend(frame, legend, s, f, hits);
}

/// Each bucket's share as an arc, with edges blended into the page and a
/// dark gap between arcs. A selected bucket's arc lifts out of the ring, the
/// rest fade, and its largest items ring it, the one under the cursor lit.
fn render_donut(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env) {
    let t = f.theme;
    let ring = Ring::new(area);
    let arcs = s.arcs();
    let bg = t.bg_rgb();
    let colours: Vec<Rgb> = s
        .buckets
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let c = t.bucket_rgb(&b.colour);
            match s.bucket {
                Some(sel) if sel != i => theme::mix(c, bg, 0.62),
                _ => c,
            }
        })
        .collect();
    let items = s.item_arcs(f.toggles.size);
    let selected = s.bucket.and_then(|i| Some((i, *arcs.get(i)?)));
    let mono = t.mono();
    widgets::smooth(frame.buffer_mut(), area, t, |x, y| {
        if let Some((i, arc)) = selected {
            let (r, turn) = ring.polar(x, y, ring.lift(arc));
            let inside = (arc.0..arc.1).contains(&turn) && clear_of_ends(turn, arc, r);
            if inside && (ring.inner..=ring.outer).contains(&r) {
                return Some(colours[i]);
            }
            if inside && (ring.items_in..=ring.items_out).contains(&r) {
                let lit = colours[i];
                return Some(
                    match items.iter().find(|(a, b, _)| (*a..*b).contains(&turn)) {
                        Some(&(a, b, k)) if clear_of_ends(turn, (a, b), r) => {
                            if k == s.cursor || mono {
                                lit
                            } else {
                                theme::mix(lit, bg, 0.45)
                            }
                        }
                        Some(_) => return None,
                        None => theme::mix(lit, bg, 0.78),
                    },
                );
            }
        }
        let (r, turn) = ring.polar(x, y, (0.0, 0.0));
        if !(ring.inner..=ring.outer).contains(&r) {
            return None;
        }
        let i = arcs.iter().position(|(_, end)| turn <= *end)?;
        if Some(i) == s.bucket || !clear_of_ends(turn, arcs[i], r) {
            return None;
        }
        if mono {
            let (px, py) = (x as u16, y as u16);
            let solid = if s.bucket.is_some() {
                false
            } else {
                i % 2 == 0
            };
            if !solid && (px + py) % 2 == 1 {
                return None;
            }
        }
        Some(colours[i])
    });
    let total: u64 = s.buckets.iter().map(|b| b.bytes).sum();
    let hole = ((ring.inner * 2.0) as usize).saturating_sub(2);
    let lines: Vec<(String, Style)> = match s.selected_bucket() {
        Some(b) if total > 0 => vec![
            (b.label.clone(), Style::new().fg(t.bucket_hex(&b.colour))),
            (human::bytes(b.bytes), t.text().add_modifier(Modifier::BOLD)),
            (
                format!("{:.1}%", b.bytes as f64 / total as f64 * 100.0),
                t.dim(),
            ),
        ],
        _ => vec![
            (human::bytes(total), t.text().add_modifier(Modifier::BOLD)),
            ("scanned".into(), t.dim()),
        ],
    };
    let rows = (ring.inner as u16).min(lines.len() as u16);
    if hole < 8 || rows < 2 {
        return;
    }
    let top = area.y + area.height / 2 - rows / 2;
    for (k, (text, style)) in lines.into_iter().take(usize::from(rows)).enumerate() {
        let text = data::truncate_middle(&text, hole);
        let w = text.chars().count() as u16;
        let r = Rect::new(area.x + (area.width - w) / 2, top + k as u16, w, 1);
        frame.render_widget(Paragraph::new(Span::styled(text, style)), r);
    }
}

fn render_legend(
    frame: &mut Frame,
    area: Rect,
    s: &OverviewState,
    f: &Env,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let t = f.theme;
    let total: u64 = s.buckets.iter().map(|b| b.bytes).sum();
    let rows = area.height as usize;
    let w = area.width as usize;
    // Names come first: the percentage goes before a name is cut short.
    let longest = s
        .buckets
        .iter()
        .map(|b| b.label.chars().count())
        .max()
        .unwrap_or(6)
        .clamp(6, 18);
    let pct_w = if w >= 2 + longest + 9 + 7 { 7 } else { 0 };
    let label_w = w.saturating_sub(2 + 9 + pct_w).clamp(6, longest);
    let bar_w = w.saturating_sub(2 + label_w + 9 + pct_w + 1).min(24);
    let max = s.buckets.iter().map(|b| b.bytes).max().unwrap_or(1).max(1);
    let shown = if s.buckets.len() > rows {
        rows.saturating_sub(1)
    } else {
        s.buckets.len()
    };
    let mut lines: Vec<Line> = Vec::new();
    for (i, b) in s.buckets.iter().enumerate().take(shown) {
        let selected = s.bucket == Some(i);
        let pct = if total == 0 {
            0.0
        } else {
            b.bytes as f64 / total as f64 * 100.0
        };
        let base = if selected { t.selected() } else { t.text() };
        let mut spans = vec![
            Span::styled("■ ", Style::new().fg(t.bucket_hex(&b.colour))),
            Span::styled(
                format!("{:<label_w$}", data::truncate_middle(&b.label, label_w)),
                base,
            ),
            Span::styled(format!("{:>9}", human::bytes(b.bytes)), base),
        ];
        if pct_w > 0 {
            spans.push(Span::styled(format!("{pct:>6.1}%"), t.dim()));
        }
        if bar_w >= 4 {
            spans.push(Span::raw(" "));
            spans.extend(
                widgets::bar(Bar {
                    frac: b.bytes as f64 / max as f64,
                    width: bar_w,
                    colour: t.bucket_rgb(&b.colour),
                    theme: t,
                })
                .spans,
            );
        }
        hits.push((
            Rect::new(area.x, area.y + i as u16, area.width, 1),
            Hit::Bucket(Some(i)),
        ));
        lines.push(Line::from(spans));
    }
    if shown < s.buckets.len() {
        lines.push(Line::styled(
            format!("  +{} more", s.buckets.len() - shown),
            t.dim(),
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_top(
    frame: &mut Frame,
    area: Rect,
    s: &mut OverviewState,
    f: &Env,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let t = f.theme;
    let mut title = vec![Span::styled(
        "Largest items",
        t.text().add_modifier(Modifier::BOLD),
    )];
    if let Some(b) = s.selected_bucket() {
        title.push(Span::styled(
            format!(" · {}", b.label),
            Style::new().fg(t.bucket_hex(&b.colour)),
        ));
    }
    if f.toggles.stale_only {
        title.push(Span::styled(" · stale only", Style::new().fg(t.warn())));
    }
    let gap = u16::from(area.height >= 12);
    let [head, _, inner] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(gap),
        Constraint::Min(1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(Line::from(title)), head);
    if s.top.is_empty() {
        frame.render_widget(
            Paragraph::new("No categorised items match.").style(t.dim()),
            inner,
        );
        return;
    }
    let h = inner.height as usize;
    if s.cursor < s.scroll {
        s.scroll = s.cursor;
    } else if s.cursor >= s.scroll + h {
        s.scroll = s.cursor + 1 - h;
    }
    let max = s
        .top
        .iter()
        .map(|i| data::item_size(i, f.toggles.size))
        .max()
        .unwrap_or(1)
        .max(1);
    let w = inner.width as usize;
    let bar_w = if w < 60 {
        6
    } else {
        (w * 12 / 100).clamp(8, 18)
    };
    let label_w = if w >= 90 { 16 } else { 0 };
    let fixed = 1 + 2 + 10 + bar_w + 2 + 5 + if label_w > 0 { label_w + 4 } else { 0 };
    let path_w = w.saturating_sub(fixed).max(8);
    let mut lines = Vec::new();
    for (row, (i, item)) in s.top.iter().enumerate().skip(s.scroll).take(h).enumerate() {
        let size = data::item_size(item, f.toggles.size);
        let colour = t.bucket_of(&f.ctx.rules, Some(&item.bucket));
        let queued = f.queue.iter().any(|p| p.as_os_str() == item.path.as_str());
        let stale = item
            .stale_days
            .filter(|d| *d >= f.ctx.config.stale_days)
            .map(human::days)
            .unwrap_or_default();
        let mark = match (queued, i == s.cursor) {
            (true, _) => "+",
            (false, true) => "▌",
            _ => " ",
        };
        let glyph_style = match item.safety {
            Safety::Protected => t.dim(),
            class => Style::new().fg(t.rgb(t.safety_rgb(class))),
        };
        let path = data::truncate_middle(&f.ctx.tilde(&item.path), path_w);
        let cut = path.rfind('/').map_or(0, |i| i + 1);
        let (parent, leaf) = path.split_at(cut);
        let pad = path_w.saturating_sub(path.chars().count());
        let mut spans = vec![
            Span::styled(mark, Style::new().fg(t.accent())),
            Span::styled(format!("{} ", item.safety.glyph()), glyph_style),
            Span::styled(
                format!("{:>9} ", human::bytes(size)),
                t.text().add_modifier(Modifier::BOLD),
            ),
        ];
        spans.extend(
            widgets::bar(Bar {
                frac: size as f64 / max as f64,
                width: bar_w,
                colour,
                theme: t,
            })
            .spans,
        );
        spans.extend([
            Span::raw("  "),
            Span::styled(parent.to_string(), t.dim()),
            Span::styled(format!("{leaf}{}", " ".repeat(pad)), t.text()),
            Span::styled(format!(" {stale:>4}"), Style::new().fg(t.warn())),
        ]);
        if label_w > 0 {
            spans.push(Span::styled("  ● ", Style::new().fg(t.rgb(colour))));
            spans.push(Span::styled(
                data::truncate_middle(&item.bucket_label, label_w),
                t.dim(),
            ));
        }
        hits.push((
            Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
            Hit::Row(i),
        ));
        let line = Line::from(spans);
        lines.push(if i == s.cursor {
            line.style(t.selected())
        } else {
            line
        });
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use spacehog_core::model::NodeKind;

    use super::*;

    fn slice(id: &str, bytes: u64) -> BucketSlice {
        BucketSlice {
            id: id.into(),
            label: id.into(),
            colour: "#ffffff".into(),
            bytes,
        }
    }

    fn item(path: &str, size: u64) -> Item {
        Item {
            path: path.into(),
            size,
            size_human: String::new(),
            apparent_size: size,
            kind: NodeKind::Dir,
            bucket: "a".into(),
            bucket_label: "a".into(),
            rule_id: None,
            rule_label: None,
            safety: Safety::Cache,
            stale_days: None,
            project_root: None,
            regen_hint: None,
            last_modified: 0,
            node_id: 0,
        }
    }

    #[test]
    fn bucket_cycle_wraps_through_all() {
        let mut s = OverviewState {
            buckets: vec![slice("a", 1), slice("b", 1)],
            ..Default::default()
        };
        s.cycle_bucket(1);
        assert_eq!(s.bucket, Some(0));
        s.cycle_bucket(1);
        assert_eq!(s.bucket, Some(1));
        s.cycle_bucket(1);
        assert_eq!(s.bucket, None);
        s.cycle_bucket(-1);
        assert_eq!(s.bucket, Some(1));
    }

    #[test]
    fn clicking_the_donut_picks_the_arc_under_the_cell() {
        let s = OverviewState {
            buckets: vec![slice("a", 3), slice("b", 1)],
            ..Default::default()
        };
        let area = Rect::new(10, 5, 40, 20);
        assert_eq!(
            s.bucket_at(area, 40, 10),
            Some(0),
            "right side is in the first 75%"
        );
        assert_eq!(
            s.bucket_at(area, 17, 14),
            Some(1),
            "left of 12 o'clock is the last quarter"
        );
        assert_eq!(s.bucket_at(area, 10, 5), None, "a corner is off the ring");
    }

    #[test]
    fn a_selected_bucket_rings_its_items_until_they_would_overrun_it() {
        let s = OverviewState {
            buckets: vec![slice("a", 100), slice("b", 100)],
            top: vec![item("/big", 50), item("/mid", 30), item("/crumb", 1)],
            bucket: Some(0),
            ..Default::default()
        };
        let arcs = s.item_arcs(SizeMode::Disk);
        assert_eq!(
            arcs.iter().map(|a| a.2).collect::<Vec<_>>(),
            vec![0, 1],
            "a 1% item is a sliver"
        );
        assert!((arcs[0].1 - 0.25).abs() < 1e-9, "half of a half: {arcs:?}");
        assert!((arcs[1].1 - 0.40).abs() < 1e-9, "{arcs:?}");

        let over = OverviewState {
            top: vec![item("/big", 80), item("/again", 80)],
            ..s
        };
        assert_eq!(
            over.item_arcs(SizeMode::Disk).len(),
            1,
            "overlap is not drawn twice"
        );
    }

    #[test]
    fn clicking_an_item_arc_finds_the_item() {
        let s = OverviewState {
            buckets: vec![slice("a", 100), slice("b", 100)],
            top: vec![item("/big", 50), item("/mid", 30)],
            bucket: Some(0),
            ..Default::default()
        };
        let area = Rect::new(0, 0, 60, 30);
        let ring = Ring::new(area);
        let r = (ring.items_in + ring.items_out) / 2.0;
        let at = |turn: f64| {
            let shift = ring.lift((0.0, 0.5));
            let x = ring.cx + shift.0 + r * (turn * TAU).sin();
            let y = ring.cy + shift.1 - r * (turn * TAU).cos();
            (x as u16, (y / 2.0) as u16)
        };
        let (col, row) = at(0.12);
        assert_eq!(s.item_at(area, col, row, SizeMode::Disk), Some(0));
        let (col, row) = at(0.33);
        assert_eq!(s.item_at(area, col, row, SizeMode::Disk), Some(1));
        let (col, row) = at(0.45);
        assert_eq!(
            s.item_at(area, col, row, SizeMode::Disk),
            None,
            "the smaller rest"
        );
        let none = OverviewState { bucket: None, ..s };
        assert_eq!(none.item_at(area, col, row, SizeMode::Disk), None);
    }
}
