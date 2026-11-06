//! Drawing primitives every view shares: proportional bars on a track,
//! solid half-block shapes, stacked bars, and block sparklines.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::{self, Rgb, Theme};

pub struct Bar<'a> {
    pub frac: f64,
    pub width: usize,
    pub colour: Rgb,
    pub theme: &'a Theme,
}

const EIGHTHS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/// A bar filled in `colour` with eighth-cell precision on a dim track, so
/// its length reads against the full column. Without colour the track is dots.
pub fn bar(b: Bar) -> Line<'static> {
    let t = b.theme;
    let eighths = (b.frac.clamp(0.0, 1.0) * (b.width * 8) as f64).round() as usize;
    let full = (eighths / 8).min(b.width);
    let part = if full < b.width { eighths % 8 } else { 0 };
    let rest = b.width - full - usize::from(part > 0);
    let fg = t.rgb(b.colour);
    if t.mono() {
        return Line::from(vec![
            Span::raw("█".repeat(full)),
            Span::raw(EIGHTHS[part]),
            Span::raw("·".repeat(rest)),
        ]);
    }
    let track = t.rgb(track_of(t));
    Line::from(vec![
        Span::styled("█".repeat(full), Style::new().fg(fg).bg(track)),
        Span::styled(EIGHTHS[part], Style::new().fg(fg).bg(track)),
        Span::styled(" ".repeat(rest), Style::new().bg(track)),
    ])
}

/// The background a bar runs along: a step off the page in either theme.
pub fn track_of(t: &Theme) -> Rgb {
    t.track()
}

/// Paints a shape on a half-block grid: `colour_at(x, y)` is asked for every
/// pixel of a `area.width` by `2 * area.height` grid; `None` leaves it blank.
pub fn half_blocks(
    buf: &mut Buffer,
    area: Rect,
    t: &Theme,
    colour_at: impl Fn(u16, u16) -> Option<Rgb>,
) {
    for cy in 0..area.height {
        for cx in 0..area.width {
            let top = colour_at(cx, cy * 2);
            let bottom = colour_at(cx, cy * 2 + 1);
            let cell = &mut buf[(area.x + cx, area.y + cy)];
            match (top, bottom) {
                (None, None) => {}
                (Some(a), Some(b)) if a == b => {
                    cell.set_symbol("█");
                    cell.set_style(Style::new().fg(t.rgb(a)));
                }
                (Some(a), None) => {
                    cell.set_symbol("▀");
                    cell.set_style(Style::new().fg(t.rgb(a)));
                }
                (None, Some(b)) => {
                    cell.set_symbol("▄");
                    cell.set_style(Style::new().fg(t.rgb(b)));
                }
                (Some(a), Some(b)) => {
                    cell.set_symbol("▀");
                    cell.set_style(Style::new().fg(t.rgb(a)).bg(t.rgb(b)));
                }
            }
        }
    }
}

/// One labelled share of a stacked bar.
pub struct Segment {
    pub label: String,
    pub bytes: u64,
    pub colour: Rgb,
}

/// A full-width bar split by share, with labels under it. Segments too
/// narrow for their label still draw; their label is skipped.
pub fn stacked(buf: &mut Buffer, area: Rect, segs: &[Segment], t: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let total: u64 = segs.iter().map(|s| s.bytes).sum::<u64>().max(1);
    let w = f64::from(area.width);
    let mut acc = 0u64;
    let mut x0 = area.x;
    let glyphs = ["█", "▓", "▒", "░"];
    for (i, s) in segs.iter().enumerate() {
        acc += s.bytes;
        let x1 = area.x + ((acc as f64 / total as f64) * w).round() as u16;
        let width = x1.saturating_sub(x0);
        for x in x0..x1 {
            let cell = &mut buf[(x, area.y)];
            if t.mono() {
                cell.set_symbol(glyphs[i % glyphs.len()]);
            } else {
                cell.set_symbol("█");
                cell.set_style(Style::new().fg(t.rgb(s.colour)));
            }
        }
        if area.height > 1 && width >= 6 {
            let pct = s.bytes as f64 / total as f64 * 100.0;
            let short: Vec<&str> = s.label.split_whitespace().take(2).collect();
            let label = [
                format!("{} {pct:.0}%", s.label),
                format!("{} {pct:.0}%", short.join(" ")),
                format!("{} {pct:.0}%", short.first().copied().unwrap_or_default()),
            ]
            .into_iter()
            .find(|l| (l.chars().count() as u16) < width)
            .unwrap_or_default();
            buf.set_stringn(
                x0,
                area.y + 1,
                &label,
                usize::from(width.saturating_sub(1)),
                Style::new()
                    .fg(t.rgb(s.colour))
                    .add_modifier(Modifier::BOLD),
            );
        }
        x0 = x1;
    }
}

/// A filled area chart with eighth-block tops and a bright crest, the
/// latest samples at the right edge.
pub fn area_chart(buf: &mut Buffer, area: Rect, samples: &[u64], colour: Rgb, t: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let n = usize::from(area.width);
    let shown = &samples[samples.len().saturating_sub(n)..];
    if shown.is_empty() {
        return;
    }
    let max = shown.iter().copied().max().unwrap_or(0).max(1);
    let levels = f64::from(area.height) * 8.0;
    let fg = t.rgb(colour);
    let fade = t.rgb(theme::scale(colour, 0.55));
    for col in 0..n {
        // Until there are more samples than columns, each sample is stretched
        // across several columns so the chart always spans the full width.
        let v = shown[col * shown.len() / n];
        let x = area.x + col as u16;
        let filled = ((v as f64 / max as f64) * levels).round() as u16;
        let top_row = filled.div_ceil(8);
        for row in 0..area.height {
            let from_bottom = area.height - 1 - row;
            let here = filled.saturating_sub(from_bottom * 8).min(8);
            let sym = match here {
                0 => continue,
                8 => "█",
                k => ["▁", "▂", "▃", "▄", "▅", "▆", "▇"][usize::from(k) - 1],
            };
            let cell = &mut buf[(x, area.y + row)];
            cell.set_symbol(sym);
            let c = if from_bottom + 1 == top_row { fg } else { fade };
            cell.set_style(Style::new().fg(if t.mono() { Color::Reset } else { c }));
        }
    }
}

/// A centred card for empty states: a title, then lines of explanation.
pub fn empty_state(
    frame: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    t: &Theme,
) {
    use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
    let w = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(title.chars().count() + 4) as u16
        + 6;
    let w = w.min(area.width);
    let h = (lines.len() as u16 + 4).min(area.height);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.border())
        .title(Span::styled(format!(" {title} "), t.title()))
        .title_alignment(ratatui::layout::Alignment::Center);
    let mut body = vec![Line::default()];
    body.extend(lines.into_iter().map(Line::centered));
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: true }).block(block),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Tone;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn bar_is_always_its_full_width() {
        let t = Theme::new(Tone::Dark, true, false);
        for frac in [0.0, 0.01, 0.5, 0.99, 1.0, 1.5] {
            let l = bar(Bar {
                frac,
                width: 20,
                colour: (255, 0, 0),
                theme: &t,
            });
            assert_eq!(text(&l).chars().count(), 20, "{frac}");
        }
        let half = bar(Bar {
            frac: 0.5,
            width: 4,
            colour: (255, 0, 0),
            theme: &t,
        });
        assert_eq!(text(&half), "██  ");
    }
}
