use std::f64::consts::TAU;

use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use spacehog_core::human;
use spacehog_core::model::SizeMode;
use spacehog_core::report::{self, Item, RuleSummary};
use spacehog_core::rules::Safety;
use tui_big_text::{BigText, PixelSize};

use crate::app::Hit;
use crate::data::{self, BucketSlice, Ctx, SafetySplit, Snapshot, Toggles};
use crate::theme::{self, Theme};
use crate::widgets::{self, Bar, Segment};

/// The Overview's data and cursor. `bucket` is the selected donut segment,
/// which narrows the offenders list to that bucket.
#[derive(Debug, Default)]
pub struct OverviewState {
    pub buckets: Vec<BucketSlice>,
    pub top: Vec<Item>,
    pub safety: SafetySplit,
    pub rules: Vec<RuleSummary>,
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
        self.rules = report::summary(&ctx.index, &ctx.rules, snap.scan.id, ctx.config.stale_days)?
            .rules
            .into_iter()
            .filter(|r| r.bytes > 0)
            .take(24)
            .collect();
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

    /// Cumulative share where each bucket's arc ends, clockwise from 12 o'clock.
    fn bounds(&self) -> Vec<f64> {
        let total: u64 = self.buckets.iter().map(|b| b.bytes).sum();
        let mut acc = 0u64;
        self.buckets
            .iter()
            .map(|b| {
                acc += b.bytes;
                if total == 0 {
                    1.0
                } else {
                    acc as f64 / total as f64
                }
            })
            .collect()
    }

    /// The bucket whose arc is under a cell of the donut drawn in `area`.
    pub fn bucket_at(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        let g = Ring::new(area);
        let x = f64::from(col.checked_sub(area.x)?) + 0.5;
        let y = f64::from(row.checked_sub(area.y)?) * 2.0 + 1.0;
        let (r, turn) = g.polar(x, y);
        if r > g.outer + 1.0 {
            return None;
        }
        self.bounds().iter().position(|b| turn <= *b)
    }
}

/// Donut geometry in half-block pixels, which are about square.
struct Ring {
    cx: f64,
    cy: f64,
    outer: f64,
    inner: f64,
}

impl Ring {
    fn new(area: Rect) -> Self {
        let (w, h) = (f64::from(area.width), f64::from(area.height) * 2.0);
        let outer = (w.min(h) / 2.0 - 0.5).max(1.0);
        Self {
            cx: w / 2.0,
            cy: h / 2.0,
            outer,
            inner: outer * 0.58,
        }
    }

    /// Radius and turn (0..1, clockwise from 12 o'clock) of a pixel centre.
    fn polar(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (x - self.cx, y - self.cy);
        let r = (dx * dx + dy * dy).sqrt();
        (r, (dx.atan2(-dy) / TAU).rem_euclid(1.0))
    }
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
    let hero_h = if tall { 5 } else { 2 };
    let bottom_h = if area.height >= 42 {
        (area.height / 4).clamp(6, 14)
    } else {
        0
    };
    let [hero, _, safety, main, bottom] = Layout::vertical([
        Constraint::Length(hero_h),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(bottom_h),
    ])
    .areas(area);
    render_hero(frame, hero, s, f, tall);
    render_safety_bar(frame, safety, s, f);
    let left_w = if main.width >= 140 {
        main.width * 2 / 5
    } else {
        38
    };
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_w), Constraint::Min(30)]).areas(main);
    render_buckets(frame, left, s, f, hits);
    render_top(frame, right, s, f, hits);
    if bottom_h > 0 {
        render_rules(frame, bottom, s, f);
    }
}

fn total_line<'a>(s: &OverviewState, f: &Env<'a>) -> Line<'a> {
    let t = f.theme;
    let mut spans = vec![
        Span::styled(human::bytes(f.snap.root.size(f.toggles.size)), t.title()),
        Span::styled(" in ", t.dim()),
        Span::styled(f.ctx.tilde(&f.snap.root.path), t.text()),
        Span::styled("  · ", t.dim()),
        Span::styled(
            human::bytes(s.safety.safe()),
            Style::new().fg(t.good()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" safe to free", t.dim()),
    ];
    if let Some(b) = s.selected_bucket() {
        spans.push(Span::styled(
            format!("  · {}", b.label),
            Style::new().fg(t.bucket_hex(&b.colour)),
        ));
    }
    Line::from(spans)
}

/// Two headline figures, each as big as the space allows, and the volume gauge.
fn render_hero(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env, tall: bool) {
    let t = f.theme;
    let total = human::bytes(f.snap.root.size(f.toggles.size));
    let safe = human::bytes(s.safety.safe());
    let size_word = match f.toggles.size {
        SizeMode::Disk => "on disk in",
        SizeMode::Apparent => "apparent size in",
    };
    let fig_w = if tall { 4 * 9 } else { 24 };
    let [a, b, c] = Layout::horizontal([
        Constraint::Length(fig_w + 2),
        Constraint::Length(fig_w + 2),
        Constraint::Min(20),
    ])
    .areas(area);
    let figure = |frame: &mut Frame, r: Rect, value: String, style: Style, caption: Line| {
        let [num, cap] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(r);
        if tall {
            frame.render_widget(
                BigText::builder()
                    .pixel_size(PixelSize::Quadrant)
                    .style(style)
                    .lines(vec![value.into()])
                    .build(),
                num,
            );
        } else {
            frame.render_widget(Paragraph::new(Span::styled(value, style)), num);
        }
        frame.render_widget(Paragraph::new(caption), cap);
    };
    figure(
        frame,
        a,
        total,
        t.title(),
        Line::from(vec![
            Span::styled(format!("{size_word} "), t.dim()),
            Span::styled(f.ctx.tilde(&f.snap.root.path), t.text()),
        ]),
    );
    figure(
        frame,
        b,
        safe,
        Style::new().fg(t.good()).add_modifier(Modifier::BOLD),
        Line::from(vec![
            Span::styled("safe to free · ", t.dim()),
            Span::styled(human::bytes(s.safety.stale_safe), Style::new().fg(t.warn())),
            Span::styled(" stale", t.dim()),
        ]),
    );
    let lines = match &f.snap.volume {
        Some(v) => {
            let ratio = if v.total == 0 {
                0.0
            } else {
                v.used as f64 / v.total as f64
            };
            let w = usize::from(c.width.saturating_sub(1));
            let colour = if ratio > 0.9 {
                theme::scale((255, 184, 108), 1.0)
            } else {
                t.bucket_rgb("#bd93f9")
            };
            vec![
                Line::from(vec![
                    Span::styled(v.mount.clone(), t.text()),
                    Span::styled(format!("  {:.0}% used", ratio * 100.0), t.dim()),
                ]),
                widgets::bar(Bar {
                    frac: ratio,
                    width: w,
                    colour,
                    theme: t,
                }),
                Line::from(vec![
                    Span::styled(v.free_human.clone(), t.text().add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" free of {}", v.total_human), t.dim()),
                ]),
                Line::styled(
                    format!(
                        "{} files · scanned {}",
                        f.snap.scan.files,
                        f.snap.scan.finished_at.map_or("never".into(), data::ago)
                    ),
                    t.dim(),
                ),
            ]
        }
        None => vec![Line::styled("volume unknown", t.dim())],
    };
    let shown: Vec<Line> = if tall {
        lines
    } else {
        lines.into_iter().take(2).collect()
    };
    frame.render_widget(Paragraph::new(shown), c);
}

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
}

fn render_buckets(
    frame: &mut Frame,
    area: Rect,
    s: &OverviewState,
    f: &Env,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let t = f.theme;
    let block = Block::new()
        .borders(Borders::RIGHT | Borders::TOP)
        .border_style(t.border())
        .title(Span::styled(" Buckets ", t.title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (donut, legend) = if inner.width >= 64 {
        let dw = (inner.height * 2).min(inner.width * 11 / 20);
        let [a, _, b] = Layout::horizontal([
            Constraint::Length(dw),
            Constraint::Length(2),
            Constraint::Min(10),
        ])
        .areas(inner);
        (
            Rect {
                height: dw / 2,
                ..a
            },
            b,
        )
    } else {
        let legend_rows = (s.buckets.len() as u16).min(inner.height / 2);
        let dh = inner
            .height
            .saturating_sub(legend_rows + 1)
            .min(inner.width / 2)
            .max(4);
        let [a, _, b] = Layout::vertical([
            Constraint::Length(dh),
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

/// Each bucket's share as a solid arc on a half-block grid.
fn render_donut(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env) {
    let t = f.theme;
    let ring = Ring::new(area);
    let bounds = s.bounds();
    let colours: Vec<_> = s
        .buckets
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let c = t.bucket_rgb(&b.colour);
            match s.bucket {
                Some(sel) if sel != i => theme::scale(c, 0.3),
                _ => c,
            }
        })
        .collect();
    let mono = t.mono();
    widgets::half_blocks(frame.buffer_mut(), area, t, |x, y| {
        let (r, turn) = ring.polar(f64::from(x) + 0.5, f64::from(y) + 0.5);
        if r < ring.inner || r > ring.outer {
            return None;
        }
        let i = bounds.iter().position(|b| turn <= *b)?;
        if mono {
            let lit = s.bucket.is_none_or(|sel| sel == i);
            let solid = if s.bucket.is_some() { lit } else { i % 2 == 0 };
            if !solid && (x + y) % 2 == 1 {
                return None;
            }
        }
        Some(colours[i])
    });
    let total: u64 = s.buckets.iter().map(|b| b.bytes).sum();
    let (label, sub) = match s.selected_bucket() {
        Some(b) if total > 0 => (
            format!("{:.0}%", b.bytes as f64 / total as f64 * 100.0),
            b.label.clone(),
        ),
        _ => (human::bytes(total), "scanned".to_string()),
    };
    let hole = (ring.inner * 2.0) as usize;
    if hole > label.chars().count() + 1 && area.height >= 4 {
        let cy = area.y + area.height / 2;
        let put = |frame: &mut Frame, text: &str, y: u16, style: Style| {
            let w = (text.chars().count() as u16).min(area.width);
            let r = Rect::new(area.x + (area.width - w) / 2, y, w, 1);
            frame.render_widget(Paragraph::new(Span::styled(text.to_string(), style)), r);
        };
        put(frame, &label, cy.saturating_sub(1), t.title());
        if hole > sub.chars().count() + 1 {
            put(frame, &sub, cy, t.dim());
        }
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
    let num_w = 9 + 7;
    let label_w = w.saturating_sub(num_w + 2).clamp(6, 22);
    let bar_w = w.saturating_sub(label_w + num_w + 3);
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
            Span::styled(format!("{pct:>6.1}% "), t.dim()),
        ];
        if bar_w >= 4 {
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
    let mut title = vec![Span::styled(" Top offenders ", t.title())];
    if let Some(b) = s.selected_bucket() {
        title.push(Span::styled(
            format!("· {} ", b.label),
            Style::new().fg(t.bucket_hex(&b.colour)),
        ));
    }
    if f.toggles.stale_only {
        title.push(Span::styled("· stale only ", Style::new().fg(t.warn())));
    }
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(t.border())
        .title(Line::from(title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
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
    let bar_w = (w * 22 / 100).clamp(8, 36);
    let label_w = if w >= 100 { 16 } else { 0 };
    let fixed = 2 + bar_w + 10 + 6 + 2 + label_w + 1;
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
        let mut spans = vec![Span::styled(
            if queued { "+ " } else { "  " },
            Style::new().fg(t.accent()),
        )];
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
            Span::styled(format!(" {:>9}", human::bytes(size)), t.text()),
            Span::styled(format!(" {stale:>4} "), Style::new().fg(t.warn())),
            Span::styled(format!("{} ", item.safety.glyph()), t.dim()),
            Span::styled(
                format!(
                    "{:<path_w$}",
                    data::truncate_middle(&f.ctx.tilde(&item.path), path_w)
                ),
                t.text(),
            ),
        ]);
        if label_w > 0 {
            spans.push(Span::styled(
                format!(
                    " {:<label_w$}",
                    data::truncate_middle(&item.bucket_label, label_w)
                ),
                Style::new().fg(t.rgb(colour)),
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

/// The largest rules across the scan, in as many columns as the width holds.
fn render_rules(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env) {
    let t = f.theme;
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(t.border())
        .title(Span::styled(" Largest rules ", t.title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let cols = (inner.width / 56).clamp(1, 4);
    let col_areas = Layout::horizontal(vec![Constraint::Ratio(1, u32::from(cols)); cols as usize])
        .spacing(2)
        .split(inner);
    let per = inner.height as usize;
    let max = s.rules.first().map_or(1, |r| r.bytes).max(1);
    for (c, col) in col_areas.iter().enumerate() {
        let w = col.width as usize;
        let label_w = (w * 2 / 5).clamp(10, 26);
        let bar_w = w.saturating_sub(label_w + 10 + 10 + 3);
        let lines: Vec<Line> = s
            .rules
            .iter()
            .skip(c * per)
            .take(per)
            .map(|r| {
                let colour = t.bucket_of(&f.ctx.rules, Some(&r.bucket));
                let mut spans = vec![Span::styled(
                    format!(
                        "{} {:<label_w$}",
                        r.safety.glyph(),
                        data::truncate_middle(&r.label, label_w.saturating_sub(2))
                    ),
                    t.text(),
                )];
                spans.extend(
                    widgets::bar(Bar {
                        frac: r.bytes as f64 / max as f64,
                        width: bar_w,
                        colour,
                        theme: t,
                    })
                    .spans,
                );
                spans.push(Span::styled(
                    format!(" {:>9}", human::bytes(r.bytes)),
                    t.text(),
                ));
                spans.push(Span::styled(
                    format!(" {:>8}", format!("×{}", r.items)),
                    t.dim(),
                ));
                Line::from(spans)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), *col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(id: &str, bytes: u64) -> BucketSlice {
        BucketSlice {
            id: id.into(),
            label: id.into(),
            colour: "#ffffff".into(),
            bytes,
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
            s.bucket_at(area, 15, 10),
            Some(1),
            "left of 12 o'clock is the last quarter"
        );
    }
}
