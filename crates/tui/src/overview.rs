use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Points};
use ratatui::widgets::{Block, Borders, LineGauge, Paragraph};
use spacehog_core::human;
use spacehog_core::report::Item;
use tui_big_text::{BigText, PixelSize};

use crate::data::{self, BucketSlice, Ctx, Snapshot, Toggles};
use crate::explorer::bar;
use crate::theme::{self, Theme};

/// The Overview's data and cursor. `bucket` is the selected donut segment,
/// which narrows the offenders list to that bucket.
#[derive(Debug, Default)]
pub struct OverviewState {
    pub buckets: Vec<BucketSlice>,
    pub top: Vec<Item>,
    pub bucket: Option<usize>,
    pub cursor: usize,
}

impl OverviewState {
    pub fn reload(&mut self, ctx: &Ctx, snap: &Snapshot, t: Toggles, filter: &str) -> Result<()> {
        let selected_id = self.selected_bucket().map(|b| b.id.clone());
        self.buckets = data::buckets(ctx, snap, t)?;
        self.bucket = selected_id.and_then(|id| self.buckets.iter().position(|b| b.id == id));
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
}

pub struct Env<'a> {
    pub ctx: &'a Ctx,
    pub theme: &'a Theme,
    pub snap: &'a Snapshot,
    pub toggles: Toggles,
    pub queue: &'a [std::path::PathBuf],
}

pub fn render(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env) {
    if area.width < 80 {
        let [head, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
        frame.render_widget(Paragraph::new(total_line(s, f)), head);
        render_top(frame, list, s, f);
        return;
    }
    let left_w = if area.width >= 140 {
        area.width * 45 / 100
    } else {
        38
    };
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_w), Constraint::Min(30)]).areas(area);
    render_left(frame, left, s, f);
    render_top(frame, right, s, f);
}

fn total_line<'a>(s: &OverviewState, f: &Env<'a>) -> Line<'a> {
    let t = f.theme;
    let mut spans = vec![
        Span::styled(human::bytes(f.snap.root.size(f.toggles.size)), t.title()),
        Span::styled(" scanned in ", t.dim()),
        Span::styled(f.ctx.tilde(&f.snap.root.path), t.text()),
    ];
    if let Some(b) = s.selected_bucket() {
        spans.push(Span::styled(
            format!("  · {}", b.label),
            Style::new().fg(t.hex(&b.colour)),
        ));
    }
    Line::from(spans)
}

fn render_left(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env) {
    let t = f.theme;
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(t.border());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let big = inner.height >= 30;
    let [disk, gauge, _, number, caption, _, chart] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(if big { 4 } else { 1 }),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(4),
    ])
    .areas(inner);

    match &f.snap.volume {
        Some(v) => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{} ", v.mount), t.text()),
                    Span::styled(
                        format!("{} free of {}", v.free_human, v.total_human),
                        t.dim(),
                    ),
                ])),
                disk,
            );
            let ratio = if v.total == 0 {
                0.0
            } else {
                v.used as f64 / v.total as f64
            };
            frame.render_widget(
                LineGauge::default()
                    .ratio(ratio)
                    .label(format!("{:>3.0}%", ratio * 100.0))
                    .filled_style(Style::new().fg(if ratio > 0.9 { t.warn() } else { t.accent() }))
                    .unfilled_style(t.border()),
                gauge,
            );
        }
        None => frame.render_widget(Paragraph::new("volume unknown").style(t.dim()), disk),
    }

    let total = human::bytes(f.snap.root.size(f.toggles.size));
    if big {
        frame.render_widget(
            BigText::builder()
                .pixel_size(PixelSize::Quadrant)
                .style(t.title())
                .lines(vec![total.clone().into()])
                .build(),
            number,
        );
    } else {
        frame.render_widget(Paragraph::new(Span::styled(total, t.title())), number);
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                match f.toggles.size {
                    spacehog_core::model::SizeMode::Disk => "on disk in ",
                    spacehog_core::model::SizeMode::Apparent => "apparent in ",
                },
                t.dim(),
            ),
            Span::styled(f.ctx.tilde(&f.snap.root.path), t.text()),
            Span::styled(format!(" · {} files", f.snap.scan.files), t.dim()),
        ])),
        caption,
    );

    let side_by_side = chart.width >= 64;
    let (donut_area, legend_area) = if side_by_side {
        let rows = (s.buckets.len() as u16 + 2).clamp(9, 14).min(chart.height);
        let [row, _] =
            Layout::vertical([Constraint::Length(rows), Constraint::Fill(1)]).areas(chart);
        let [a, b] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(row);
        (a, b)
    } else {
        let donut_h = (chart.height / 2).clamp(4, 10);
        let [a, b] =
            Layout::vertical([Constraint::Length(donut_h), Constraint::Min(1)]).areas(chart);
        (a, b)
    };
    render_donut(frame, donut_area, s, f);
    render_legend(frame, legend_area, s, f);
}

/// Each bucket's share as an arc of a ring drawn in braille dots. Dots are
/// square when a cell is twice as tall as wide, so canvas units are dots.
fn render_donut(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env) {
    let t = f.theme;
    let (w, h) = (f64::from(area.width) * 2.0, f64::from(area.height) * 4.0);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let outer = cx.min(cy) - 0.5;
    let inner = outer * 0.58;
    let total: u64 = s.buckets.iter().map(|b| b.bytes).sum();
    let mut bounds = Vec::with_capacity(s.buckets.len());
    let mut acc = 0u64;
    for b in &s.buckets {
        acc += b.bytes;
        bounds.push(if total == 0 {
            1.0
        } else {
            acc as f64 / total as f64
        });
    }
    let mut points: Vec<Vec<(f64, f64)>> = vec![Vec::new(); s.buckets.len()];
    for yi in 0..(h as usize) {
        for xi in 0..(w as usize) {
            let (x, y) = (xi as f64 + 0.5, yi as f64 + 0.5);
            let (dx, dy) = (x - cx, y - cy);
            let r = (dx * dx + dy * dy).sqrt();
            if r < inner || r > outer {
                continue;
            }
            let turn = (dx.atan2(dy) / std::f64::consts::TAU).rem_euclid(1.0);
            if let Some(i) = bounds.iter().position(|b| turn <= *b) {
                points[i].push((x, y));
            }
        }
    }
    let colours: Vec<_> = s
        .buckets
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let c = t.hex(&b.colour);
            match s.bucket {
                Some(sel) if sel != i => theme::dim(c),
                _ => c,
            }
        })
        .collect();
    let label = match s.selected_bucket() {
        Some(b) if total > 0 => format!("{:.0}%", b.bytes as f64 / total as f64 * 100.0),
        _ => human::bytes(total),
    };
    let fits = inner as usize > label.chars().count() + 1;
    let label_style = t.title();
    let canvas = Canvas::default()
        .x_bounds([0.0, w])
        .y_bounds([0.0, h])
        .paint(move |ctx| {
            for (pts, colour) in points.iter().zip(&colours) {
                ctx.draw(&Points {
                    coords: pts,
                    color: *colour,
                });
            }
            if fits {
                let x = cx - label.chars().count() as f64;
                ctx.print(x, cy, Span::styled(label.clone(), label_style));
            }
        });
    frame.render_widget(canvas, area);
}

fn render_legend(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env) {
    let t = f.theme;
    let total: u64 = s.buckets.iter().map(|b| b.bytes).sum();
    let rows = area.height as usize;
    let mut lines: Vec<Line> = s
        .buckets
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let selected = s.bucket == Some(i);
            let pct = if total == 0 {
                0.0
            } else {
                b.bytes as f64 / total as f64 * 100.0
            };
            let label_w = (area.width as usize).saturating_sub(20).clamp(6, 18);
            let base = if selected { t.selected() } else { t.text() };
            Line::from(vec![
                Span::styled("■ ", Style::new().fg(t.hex(&b.colour))),
                Span::styled(
                    format!("{:<label_w$}", data::truncate_middle(&b.label, label_w)),
                    base,
                ),
                Span::styled(format!("{:>9}", human::bytes(b.bytes)), base),
                Span::styled(format!("{pct:>6.1}%"), t.dim()),
            ])
        })
        .collect();
    if lines.len() > rows {
        let hidden = lines.len() - rows + 1;
        lines.truncate(rows - 1);
        lines.push(Line::styled(format!("  +{hidden} more"), t.dim()));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_top(frame: &mut Frame, area: Rect, s: &OverviewState, f: &Env) {
    let t = f.theme;
    let mut title = vec![Span::styled(" Top offenders ", t.title())];
    if let Some(b) = s.selected_bucket() {
        title.push(Span::styled(
            format!("· {} ", b.label),
            Style::new().fg(t.hex(&b.colour)),
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
    let max = s
        .top
        .iter()
        .map(|i| data::item_size(i, f.toggles.size))
        .max()
        .unwrap_or(1)
        .max(1);
    let bar_w = 8;
    let fixed = 2 + bar_w + 10 + 6 + 3;
    let path_w = (inner.width as usize).saturating_sub(fixed).max(8);
    let lines: Vec<Line> = s
        .top
        .iter()
        .enumerate()
        .take(inner.height as usize)
        .map(|(i, item)| {
            let size = data::item_size(item, f.toggles.size);
            let colour = t.bucket(&f.ctx.rules, Some(&item.bucket));
            let queued = f.queue.iter().any(|p| p.as_os_str() == item.path.as_str());
            let stale = item
                .stale_days
                .filter(|d| *d >= f.ctx.config.stale_days)
                .map(human::days)
                .unwrap_or_default();
            let line = Line::from(vec![
                Span::styled(
                    if queued { "+ " } else { "  " },
                    Style::new().fg(t.accent()),
                ),
                Span::styled(
                    bar(size as f64 / max as f64, bar_w),
                    Style::new().fg(colour),
                ),
                Span::styled(format!(" {:>9}", human::bytes(size)), t.text()),
                Span::styled(format!(" {stale:>4} "), Style::new().fg(t.warn())),
                Span::styled(format!("{} ", item.safety.glyph()), t.dim()),
                Span::styled(
                    data::truncate_middle(&f.ctx.tilde(&item.path), path_w),
                    t.text(),
                ),
            ]);
            if i == s.cursor {
                line.style(t.selected())
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(id: &str) -> BucketSlice {
        BucketSlice {
            id: id.into(),
            label: id.into(),
            colour: "#ffffff".into(),
            bytes: 1,
        }
    }

    #[test]
    fn bucket_cycle_wraps_through_all() {
        let mut s = OverviewState {
            buckets: vec![slice("a"), slice("b")],
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
}
