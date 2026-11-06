use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use spacehog_core::engine::{ScanJob, ScanOutcome};
use spacehog_core::human;
use spacehog_core::scan::{Phase, ProgressSnapshot, RateMeter, TopLevelDir};
use throbber_widgets_tui::{BRAILLE_SIX, Throbber, ThrobberState};
use tui_big_text::{BigText, PixelSize};

use crate::data::{Ctx, truncate_middle};
use crate::theme::Theme;
use crate::widgets::{self, Bar};

/// A running scan plus what the scan screen draws from it. `snapshot`, `elapsed`
/// and the meter refresh on each tick so drawing never reads the clock.
pub struct ScanTask {
    job: ScanJob,
    pub root: PathBuf,
    pub meter: RateMeter,
    pub throbber: ThrobberState,
    pub snapshot: ProgressSnapshot,
    /// Direct children of the root that finished walking, largest first.
    pub top: Vec<TopLevelDir>,
    pub elapsed: Duration,
    started: Instant,
}

impl ScanTask {
    pub fn new(job: ScanJob) -> Self {
        Self {
            root: job.root.clone(),
            job,
            meter: RateMeter::new(400),
            throbber: ThrobberState::default(),
            snapshot: ProgressSnapshot::default(),
            top: Vec::new(),
            elapsed: Duration::ZERO,
            started: Instant::now(),
        }
    }

    pub fn tick(&mut self) {
        self.snapshot = self.job.progress.snapshot();
        self.top = self.job.progress.finished_top_level();
        self.meter.sample(self.snapshot.files);
        self.throbber.calc_next();
        self.elapsed = self.started.elapsed();
    }

    pub fn is_finished(&self) -> bool {
        self.job.is_finished()
    }

    pub fn join(self) -> spacehog_core::Result<ScanOutcome> {
        self.job.join()
    }

    pub fn cancel(&self) {
        self.job.progress.cancel();
    }

    pub fn rate(&self) -> u64 {
        self.meter.samples.last().copied().unwrap_or(0)
    }
}

fn phase_label(p: Phase) -> &'static str {
    match p {
        Phase::Walking => "walking",
        Phase::Labelling => "categorising",
        Phase::Writing => "indexing",
        Phase::Done => "done",
    }
}

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// One-line badge for the status strip while a scan runs in the background.
pub fn badge(task: &ScanTask) -> String {
    format!(
        "scanning… {} files {}",
        count(task.snapshot.files),
        clock(task.elapsed)
    )
}

pub fn render_screen(frame: &mut Frame, area: Rect, task: &ScanTask, ctx: &Ctx, theme: &Theme) {
    let t = theme;
    frame.render_widget(Clear, area);
    if let Some(base) = t.base() {
        frame.buffer_mut().set_style(area, base);
    }
    let snap = &task.snapshot;
    let tall = area.height >= 30;
    let big_h = if area.height >= 24 { 4 } else { 1 };
    let chart_h = if tall {
        (area.height / 5).clamp(4, 10)
    } else {
        3
    };
    let [head, _, big, tiles, steps, _, chart, path, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(big_h),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(chart_h),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(area.inner(Margin::new(1, 0)));

    let throbber = Throbber::default()
        .throbber_set(BRAILLE_SIX)
        .throbber_style(Style::new().fg(t.accent()));
    let mut spans = vec![
        throbber.to_symbol_span(&task.throbber),
        Span::styled(" Scanning ", t.title()),
        Span::styled(ctx.tilde(&task.root.to_string_lossy()), t.text()),
    ];
    let hint = Line::from(vec![
        Span::styled("esc", t.title()),
        Span::styled(" run in background", t.dim()),
    ]);
    spans.push(Span::raw(" "));
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(hint.width() as u16)])
        .areas(head);
    frame.render_widget(Paragraph::new(Line::from(spans)), l);
    frame.render_widget(Paragraph::new(hint), r);

    let bytes = human::bytes(snap.bytes);
    if big_h == 4 {
        let size = if big.width >= bytes.len() as u16 * 8 + 20 {
            PixelSize::HalfHeight
        } else {
            PixelSize::Quadrant
        };
        let [num, cap] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(26)]).areas(big);
        frame.render_widget(
            BigText::builder()
                .pixel_size(size)
                .style(t.title())
                .lines(vec![bytes.into()])
                .build(),
            num,
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::default(),
                Line::styled("found so far", t.dim()),
                Line::from(vec![
                    Span::styled(count(task.rate()), t.text().add_modifier(Modifier::BOLD)),
                    Span::styled(" files/s", t.dim()),
                ]),
            ]),
            cap,
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(bytes, t.title()),
                Span::styled(format!("  {} files/s", count(task.rate())), t.dim()),
            ])),
            big,
        );
    }

    let stats = [
        ("files", count(snap.files)),
        ("folders", count(snap.dirs)),
        ("errors", count(snap.errors)),
        ("elapsed", clock(task.elapsed)),
    ];
    let cells = Layout::horizontal([Constraint::Ratio(1, 4); 4])
        .spacing(1)
        .split(tiles);
    for ((label, value), cell) in stats.iter().zip(cells.iter()) {
        let warn = *label == "errors" && snap.errors > 0;
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(t.border())
            .title(Span::styled(format!(" {label} "), t.dim()));
        frame.render_widget(
            Paragraph::new(Span::styled(
                value.clone(),
                if warn {
                    Style::new().fg(t.warn()).add_modifier(Modifier::BOLD)
                } else {
                    t.text().add_modifier(Modifier::BOLD)
                },
            ))
            .block(block),
            *cell,
        );
    }

    let mut stepper = Vec::new();
    for (i, p) in [Phase::Walking, Phase::Labelling, Phase::Writing]
        .into_iter()
        .enumerate()
    {
        let (dot, style) = if p == snap.phase {
            (
                "●",
                Style::new().fg(t.accent()).add_modifier(Modifier::BOLD),
            )
        } else if (p as u8) < (snap.phase as u8) {
            ("✓", Style::new().fg(t.good()))
        } else {
            ("○", t.dim())
        };
        if i > 0 {
            stepper.push(Span::styled(" ──── ", t.border()));
        }
        stepper.push(Span::styled(format!("{dot} {}", phase_label(p)), style));
    }
    if snap.phase == Phase::Writing {
        stepper.push(Span::styled(
            format!("   {} rows written", count(snap.written)),
            t.text(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(stepper)), steps);

    widgets::area_chart(
        frame.buffer_mut(),
        chart,
        &task.meter.samples,
        t.bucket_rgb("#ff79c6"),
        t,
    );
    let current = ctx.tilde(&snap.current_path);
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate_middle(&current, path.width as usize),
            t.dim(),
        )),
        path,
    );

    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(t.border())
        .title(Span::styled(" Largest folders so far ", t.title()));
    let inner = block.inner(list);
    frame.render_widget(block, list);
    if task.top.is_empty() {
        frame.render_widget(
            Paragraph::new("Folders appear here as each one finishes.").style(t.dim()),
            inner,
        );
        return;
    }
    let max = task.top[0].disk.max(1);
    let w = inner.width as usize;
    let longest = task
        .top
        .iter()
        .map(|d| d.name.chars().count())
        .max()
        .unwrap_or(8);
    let name_w = longest.clamp(8, 40).min(w / 3);
    let bar_w = w.saturating_sub(name_w + 10 + 16 + 2).max(4);
    let lines: Vec<Line> = task
        .top
        .iter()
        .take(inner.height as usize)
        .map(|d| {
            let mut spans = vec![Span::styled(
                format!("{:<name_w$} ", truncate_middle(&d.name, name_w)),
                t.text().add_modifier(Modifier::BOLD),
            )];
            spans.extend(
                widgets::bar(Bar {
                    frac: d.disk as f64 / max as f64,
                    width: bar_w,
                    colour: t.bucket_rgb("#bd93f9"),
                    theme: t,
                })
                .spans,
            );
            spans.push(Span::styled(
                format!(" {:>9}", human::bytes(d.disk)),
                t.text(),
            ));
            spans.push(Span::styled(
                format!(" {:>14}", format!("{} files", count(d.files))),
                t.dim(),
            ));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_group_thousands() {
        assert_eq!(super::count(1_234_567), "1,234,567");
        assert_eq!(super::count(999), "999");
        assert_eq!(super::count(1_000), "1,000");
    }
}
