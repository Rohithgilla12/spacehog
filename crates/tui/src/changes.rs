use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use spacehog_core::changes::{self, ChangeKind, Changes, PathDelta, Since};
use spacehog_core::model::ScanRecord;

use crate::data::{self, Ctx, truncate_middle};
use crate::theme::Theme;
use crate::widgets::{self, Bar};

pub const LIMIT: usize = 30;

/// Kept scans of the session's root (newest first) and the diff from the
/// chosen baseline to the newest. Core always diffs against the newest scan.
#[derive(Default)]
pub struct ChangesState {
    pub scans: Vec<ScanRecord>,
    /// Index into `scans` of the older side; 0 means not chosen yet.
    pub base: usize,
    pub diff: Option<Changes>,
    pub scroll: usize,
    loaded_for: Option<i64>,
}

impl ChangesState {
    /// Loads the scan list and the diff, once per newest scan.
    pub fn ensure(&mut self, ctx: &Ctx, root: &str, latest: i64) -> Result<()> {
        if self.loaded_for == Some(latest) {
            return Ok(());
        }
        self.loaded_for = Some(latest);
        self.scans = ctx.index.scans_for_root(root)?;
        self.base = 1.min(self.scans.len().saturating_sub(1));
        self.reload(ctx, root)
    }

    fn reload(&mut self, ctx: &Ctx, root: &str) -> Result<()> {
        self.scroll = 0;
        self.diff = match self.scans.get(self.base) {
            Some(from) if self.base > 0 => Some(changes::changes(
                &ctx.index,
                &ctx.rules,
                Some(root),
                Since::Scan(from.id),
                LIMIT,
            )?),
            _ => None,
        };
        Ok(())
    }

    /// Moves the baseline older (+1) or newer (-1) among kept scans.
    pub fn shift(&mut self, ctx: &Ctx, root: &str, delta: isize) -> Result<bool> {
        let next = self.base.saturating_add_signed(delta);
        if next == 0 || next >= self.scans.len() || next == self.base {
            return Ok(false);
        }
        self.base = next;
        self.reload(ctx, root)?;
        Ok(true)
    }

    pub fn step(&mut self, delta: isize) {
        let n = self
            .diff
            .as_ref()
            .map_or(0, |d| d.grown.len().max(d.shrunk.len()));
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(n.saturating_sub(1));
    }
}

fn scan_label(s: &ScanRecord) -> String {
    format!(
        "#{} {}",
        s.id,
        s.finished_at.map_or("unfinished".into(), data::ago)
    )
}

pub fn render(frame: &mut Frame, area: Rect, s: &ChangesState, ctx: &Ctx, t: &Theme) {
    let Some(d) = &s.diff else {
        let root = s
            .scans
            .first()
            .map_or_else(|| "this folder".to_string(), |x| ctx.tilde(&x.root));
        widgets::empty_state(
            frame,
            area,
            "Nothing to compare yet",
            vec![
                Line::styled(
                    format!("Only one scan of {root} is kept. Rescan later to see what changed."),
                    t.text(),
                ),
                Line::default(),
                Line::from(vec![
                    Span::styled("r", t.title()),
                    Span::styled(" rescans now; spacehog keeps the last ", t.dim()),
                    Span::styled(ctx.config.keep_scans.to_string(), t.text()),
                    Span::styled(" scans of each folder.", t.dim()),
                ]),
            ],
            t,
        );
        return;
    };
    let grow = t.warn();
    let shrink = t.good();
    let head_line = Line::from(vec![
        Span::styled(" Changes ", t.title()),
        Span::styled(format!("in {}  ", ctx.tilde(&d.root)), t.dim()),
        Span::styled(scan_label(&d.from), t.text()),
        Span::styled(" → ", t.dim()),
        Span::styled(scan_label(&d.to), t.text()),
        Span::styled("   total ", t.dim()),
        Span::styled(
            d.total_delta_human.clone(),
            Style::new()
                .fg(if d.total_delta > 0 { grow } else { shrink })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "   [ ] older / newer baseline · {} of {} earlier scans",
                s.base,
                s.scans.len() - 1
            ),
            t.dim(),
        ),
    ]);
    let not_root = |r: &&PathDelta| r.path != d.root;
    let grown: Vec<PathDelta> = d.grown.iter().filter(not_root).cloned().collect();
    let shrunk: Vec<PathDelta> = d.shrunk.iter().filter(not_root).cloned().collect();
    let side_by_side = area.width >= 110;
    let list_rows = if side_by_side {
        grown.len().max(shrunk.len()) as u16 + 1
    } else {
        (grown.len() + shrunk.len()) as u16 + 2
    }
    .max(3);
    let history_h = if area.height >= 30 {
        (s.scans.len() as u16 + 2).clamp(4, 10)
    } else {
        0
    };
    let buckets = (d.buckets.len() as u16).max(1);
    let spare = area
        .height
        .saturating_sub(2 + buckets + 2 + list_rows + history_h);
    // Thicker bars when the terminal has room, so a short diff still fills it.
    let bar_h: u16 = if spare >= buckets * 2 {
        3
    } else if spare >= buckets {
        2
    } else {
        1
    };
    let chart_rows = (buckets * bar_h).min((area.height / 2).max(3));
    let [head, chart, lists, history, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(chart_rows + 2),
        Constraint::Min(list_rows.min(area.height / 3).max(3)),
        Constraint::Length(history_h),
        Constraint::Fill(1),
    ])
    .areas(area);
    if history_h > 0 {
        render_history(frame, history, s, t);
    }
    frame.render_widget(Paragraph::new(head_line), head);

    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(t.border())
        .title(Span::styled(" Growth by bucket ", t.title()));
    let inner = block.inner(chart);
    frame.render_widget(block, chart);
    let w = inner.width as usize;
    let label_w = 16.min(w / 4);
    let num_w = 10;
    let bar_w = w.saturating_sub(label_w + num_w + 4);
    let max = d
        .buckets
        .iter()
        .map(|b| b.delta.unsigned_abs())
        .max()
        .unwrap_or(1)
        .max(1);
    let mut lines = Vec::new();
    if d.buckets.is_empty() {
        lines.push(Line::styled("No bucket moved by more than 1 MB.", t.dim()));
    }
    for b in d
        .buckets
        .iter()
        .take((inner.height / bar_h).max(1) as usize)
    {
        let colour = t.bucket_rgb(&b.colour);
        let mut spans = vec![
            Span::styled(
                format!("{:<label_w$} ", truncate_middle(&b.label, label_w)),
                t.text(),
            ),
            Span::styled(
                format!("{:>num_w$} ", b.delta_human),
                Style::new().fg(if b.delta > 0 { grow } else { shrink }),
            ),
        ];
        let bar = widgets::bar(Bar {
            frac: b.delta.unsigned_abs() as f64 / max as f64,
            width: bar_w,
            colour,
            theme: t,
        });
        spans.extend(bar.spans.clone());
        lines.push(Line::from(spans));
        if bar_h >= 2 {
            let mut pad = vec![Span::raw(" ".repeat(label_w + num_w + 2))];
            pad.extend(bar.spans.clone());
            lines.push(Line::from(pad));
        }
        if bar_h == 3 {
            lines.push(Line::default());
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let (a, b) = if side_by_side {
        let [a, b] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(lists);
        (a, b)
    } else {
        let [a, b] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(lists);
        (a, b)
    };
    path_list(frame, a, "Grew", &grown, s.scroll, grow, ctx, t);
    path_list(frame, b, "Shrank", &shrunk, s.scroll, shrink, ctx, t);
}

/// Every kept scan of the root with its total, marking the two compared.
fn render_history(frame: &mut Frame, area: Rect, s: &ChangesState, t: &Theme) {
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(t.border())
        .title(Span::styled(" Kept scans ", t.title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let max = s.scans.iter().map(|x| x.bytes).max().unwrap_or(1).max(1);
    let w = inner.width as usize;
    let bar_w = w.saturating_sub(40).max(4);
    let lines: Vec<Line> = s
        .scans
        .iter()
        .enumerate()
        .take(inner.height as usize)
        .map(|(i, scan)| {
            let (mark, style) = match i {
                0 => ("to  ", t.title()),
                i if i == s.base => ("from", t.title()),
                _ => ("    ", t.dim()),
            };
            let mut spans = vec![
                Span::styled(format!("{mark} "), style),
                Span::styled(format!("{:<18}", scan_label(scan)), t.text()),
            ];
            spans.extend(
                widgets::bar(Bar {
                    frac: scan.bytes as f64 / max as f64,
                    width: bar_w,
                    colour: t.data_rgb(),
                    theme: t,
                })
                .spans,
            );
            spans.push(Span::styled(
                format!(" {:>9}", spacehog_core::human::bytes(scan.bytes)),
                t.text(),
            ));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[allow(clippy::too_many_arguments)]
fn path_list(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    rows: &[PathDelta],
    scroll: usize,
    colour: ratatui::style::Color,
    ctx: &Ctx,
    t: &Theme,
) {
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(t.border())
        .title(Span::styled(
            format!(" {title} ({}) ", rows.len()),
            t.title(),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if rows.is_empty() {
        frame.render_widget(Paragraph::new("Nothing.").style(t.dim()), inner);
        return;
    }
    let w = inner.width as usize;
    let max = rows
        .iter()
        .map(|r| r.delta.unsigned_abs())
        .max()
        .unwrap_or(1)
        .max(1);
    let bar_w = (w / 6).clamp(4, 24);
    let path_w = w.saturating_sub(bar_w + 10 + 6 + 3);
    let start = scroll.min(rows.len().saturating_sub(inner.height as usize));
    let lines: Vec<Line> = rows
        .iter()
        .skip(start)
        .take(inner.height as usize)
        .map(|r| {
            let tag = match r.change {
                ChangeKind::New => "new ",
                ChangeKind::Gone => "gone",
                _ => "    ",
            };
            let mut spans = widgets::bar(Bar {
                frac: r.delta.unsigned_abs() as f64 / max as f64,
                width: bar_w,
                colour: t.bucket_of(&ctx.rules, r.bucket.as_deref()),
                theme: t,
            })
            .spans;
            spans.push(Span::styled(
                format!(" {:>9} ", r.delta_human),
                Style::new().fg(colour),
            ));
            spans.push(Span::styled(format!("{tag} "), t.dim()));
            spans.push(Span::styled(
                truncate_middle(&ctx.tilde(&r.path), path_w),
                t.text(),
            ));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
