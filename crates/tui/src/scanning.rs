use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use spacehog_core::engine::{ScanJob, ScanOutcome};
use spacehog_core::human;
use spacehog_core::scan::{Phase, ProgressSnapshot, RateMeter};
use throbber_widgets_tui::{BRAILLE_SIX, Throbber, ThrobberState};

use crate::data::{Ctx, truncate_middle};
use crate::theme::Theme;

/// A running scan plus what the modal draws from it. `snapshot`, `elapsed`
/// and the meter refresh on each tick so drawing never reads the clock.
pub struct ScanTask {
    job: ScanJob,
    pub root: PathBuf,
    pub meter: RateMeter,
    pub throbber: ThrobberState,
    pub snapshot: ProgressSnapshot,
    pub elapsed: Duration,
    started: Instant,
}

impl ScanTask {
    pub fn new(job: ScanJob) -> Self {
        Self {
            root: job.root.clone(),
            job,
            meter: RateMeter::new(120),
            throbber: ThrobberState::default(),
            snapshot: ProgressSnapshot::default(),
            elapsed: Duration::ZERO,
            started: Instant::now(),
        }
    }

    pub fn tick(&mut self) {
        self.snapshot = self.job.progress.snapshot();
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

pub fn render_modal(frame: &mut Frame, area: Rect, task: &ScanTask, ctx: &Ctx, theme: &Theme) {
    let w = area.width.saturating_sub(4).min(76);
    let h = 13.min(area.height);
    let modal = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, modal);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent()))
        .title(Span::styled(" Scanning ", theme.title()))
        .title_bottom(Line::from(vec![
            Span::styled(" esc ", theme.title()),
            Span::styled("run in background ", theme.dim()),
        ]));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);
    let [head, path, _, counters, phases, _, spark] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);

    let snap = &task.snapshot;
    let throbber = Throbber::default()
        .throbber_set(BRAILLE_SIX)
        .throbber_style(Style::new().fg(theme.accent()));
    let [spin, root, time] = Layout::horizontal([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(6),
    ])
    .areas(head);
    frame.render_widget(
        Paragraph::new(throbber.to_symbol_span(&task.throbber)),
        spin,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            ctx.tilde(&task.root.to_string_lossy()),
            theme.text(),
        )),
        root,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(clock(task.elapsed), theme.dim())),
        time,
    );
    let current = ctx.tilde(&snap.current_path);
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate_middle(&current, path.width as usize),
            theme.dim(),
        )),
        path,
    );

    let stat = |label: &'static str, value: String| {
        vec![
            Span::styled(format!("{label:<7}"), theme.dim()),
            Span::styled(format!("{value:<16}"), theme.text()),
        ]
    };
    let mut row1 = stat("files", count(snap.files));
    row1.extend(stat("dirs", count(snap.dirs)));
    let mut row2 = stat("bytes", human::bytes(snap.bytes));
    row2.extend(stat("errors", count(snap.errors)));
    frame.render_widget(
        Paragraph::new(vec![Line::from(row1), Line::from(row2)]),
        counters,
    );

    let mut steps = vec![Span::styled(format!("{:<7}", "phase"), theme.dim())];
    for p in [Phase::Walking, Phase::Labelling, Phase::Writing] {
        let (dot, style) = if p == snap.phase {
            ("● ", Style::new().fg(theme.accent()))
        } else if (p as u8) < (snap.phase as u8) {
            ("✓ ", theme.dim())
        } else {
            ("○ ", theme.dim())
        };
        steps.push(Span::styled(dot, style));
        steps.push(Span::styled(format!("{}  ", phase_label(p)), style));
    }
    if snap.phase == Phase::Writing {
        steps.push(Span::styled(
            format!("{} rows written", count(snap.written)),
            theme.text(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(steps)), phases);

    let [graph, rate] =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(18)]).areas(spark);
    render_sparkline(frame, graph, &task.meter.samples, theme);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(format!(" {}", count(task.rate())), theme.title()),
            Line::styled(" files/s", theme.dim()),
        ]),
        rate,
    );
}

/// Files per second as a braille line, newest sample on the right.
fn render_sparkline(frame: &mut Frame, area: Rect, samples: &[u64], theme: &Theme) {
    let w = f64::from(area.width) * 2.0;
    let h = f64::from(area.height) * 4.0;
    let shown = samples.len().min(area.width as usize * 2);
    let tail = &samples[samples.len() - shown..];
    let max = tail.iter().copied().max().unwrap_or(1).max(1) as f64;
    let colour = theme.accent();
    let pts: Vec<(f64, f64)> = tail
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = w - (shown - i) as f64 + 0.5;
            (x, *v as f64 / max * (h - 1.0) + 0.5)
        })
        .collect();
    let canvas = Canvas::default()
        .x_bounds([0.0, w])
        .y_bounds([0.0, h])
        .paint(move |ctx| {
            for pair in pts.windows(2) {
                ctx.draw(&CanvasLine {
                    x1: pair[0].0,
                    y1: pair[0].1,
                    x2: pair[1].0,
                    y2: pair[1].1,
                    color: colour,
                });
            }
        });
    frame.render_widget(canvas, area);
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
