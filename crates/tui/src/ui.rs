use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use spacehog_core::human;
use spacehog_core::model::SizeMode;
use spacehog_core::report::Explanation;
use tui_input::Input;

use crate::app::{App, Overlay, Palette};
use crate::explorer::{self, RowFilter};
use crate::keys::{self, View};
use crate::overview::{self, Env};
use crate::{data, scanning, splash};

/// Draws the whole screen from `app`. The only mutation is the tree's scroll
/// offset, which the tree widget keeps in its own state.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if app.session.is_none() && app.scan.is_none() {
        render_picker(frame, area, app);
        return;
    }
    let [header, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    render_header(frame, header, app);
    render_body(frame, body, app);
    render_status(frame, status, app);

    match &app.overlay {
        Some(Overlay::Help) => render_help(frame, area, app),
        Some(Overlay::Palette(p)) => render_palette(frame, area, app, p),
        Some(Overlay::Info(e)) => render_info(frame, area, app, e),
        Some(Overlay::Scan) => {
            if let Some(task) = &app.scan {
                scanning::render_modal(frame, area, task, &app.ctx, &app.theme);
            }
        }
        Some(Overlay::Filter) => render_input_line(frame, status, app, "/", &app.filter),
        None => {}
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let mut spans = Vec::new();
    if area.width >= 90 {
        spans.push(Span::styled(" spacehog ", t.title()));
        spans.push(Span::styled("│", t.border()));
    }
    for v in View::ALL {
        let style = if v == app.view {
            t.selected().fg(t.accent())
        } else if v.is_built() {
            t.text()
        } else {
            t.dim()
        };
        spans.push(Span::styled(
            format!(" {} {} ", v.number(), v.label()),
            style,
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_body(frame: &mut Frame, area: Rect, app: &mut App) {
    let t = &app.theme;
    let Some(session) = app.session.as_mut() else {
        let [_, logo, tag, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(splash::logo_height()),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(area);
        let w = splash::logo_width().min(logo.width);
        let logo = Rect {
            x: logo.x + (logo.width - w) / 2,
            width: w,
            ..logo
        };
        splash::render_logo(logo, frame.buffer_mut());
        frame.render_widget(
            Paragraph::new("Indexing your first scan. Press esc to watch it in the background.")
                .style(t.dim())
                .centered(),
            tag,
        );
        return;
    };
    match app.view {
        View::Overview => overview::render(
            frame,
            area,
            &session.overview,
            &Env {
                ctx: &app.ctx,
                theme: t,
                snap: &session.snap,
                toggles: app.toggles,
                queue: &app.queue,
            },
        ),
        View::Explorer => explorer::render(
            frame,
            area,
            &mut session.explorer,
            &app.ctx,
            t,
            RowFilter {
                toggles: app.toggles,
                text: app.filter.value(),
                stale_days: app.ctx.config.stale_days,
            },
        ),
        v => frame.render_widget(
            Paragraph::new(vec![
                Line::default(),
                Line::styled(format!("{} is not built yet.", v.label()), t.title()),
                Line::styled("It arrives in a later milestone. Press 1 or 2.", t.dim()),
            ])
            .centered(),
            area,
        ),
    }
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let sep = || Span::styled(" │ ", t.border());
    let mut left = Vec::new();
    let (count, bytes) = app.proposals;
    left.push(if count > 0 {
        Span::styled(
            format!(
                " {count} proposal{} · {} ",
                if count == 1 { "" } else { "s" },
                human::bytes(bytes)
            ),
            Style::new().fg(t.hex("#282a36")).bg(t.accent()),
        )
    } else {
        Span::styled(" no proposals", t.dim())
    });
    if let Some(s) = &app.session {
        left.push(sep());
        let when = s.snap.scan.finished_at.map_or("never".into(), data::ago);
        left.push(Span::styled(format!("scanned {when}"), t.dim()));
    }
    if let Some(task) = &app.scan
        && !matches!(app.overlay, Some(Overlay::Scan))
    {
        left.push(sep());
        left.push(Span::styled(
            scanning::badge(task),
            Style::new().fg(t.accent()),
        ));
    }
    if !app.queue.is_empty() {
        left.push(sep());
        left.push(Span::styled(
            format!("{} queued", app.queue.len()),
            Style::new().fg(t.accent()),
        ));
    }
    let mut modes = Vec::new();
    if app.toggles.size == SizeMode::Apparent {
        modes.push("apparent");
    }
    if app.toggles.stale_only {
        modes.push("stale");
    }
    if !app.toggles.hidden {
        modes.push("no hidden");
    }
    if !app.filter.value().is_empty() {
        left.push(sep());
        left.push(Span::styled(format!("/{}", app.filter.value()), t.text()));
    }
    if !modes.is_empty() {
        left.push(sep());
        left.push(Span::styled(modes.join(" "), Style::new().fg(t.warn())));
    }
    if let Some(msg) = &app.flash {
        left.push(sep());
        left.push(Span::styled(msg.clone(), t.text()));
    }
    let hint = Line::from(vec![
        Span::styled("r", t.title()),
        Span::styled(" rescan  ", t.dim()),
        Span::styled("?", t.title()),
        Span::styled(" help ", t.dim()),
    ]);
    let hint_w = hint.width() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(hint_w)]).areas(area);
    frame.render_widget(Paragraph::new(Line::from(left)), l);
    frame.render_widget(Paragraph::new(hint), r);
}

fn render_input_line(frame: &mut Frame, area: Rect, app: &App, prompt: &str, input: &Input) {
    let t = &app.theme;
    frame.render_widget(Clear, area);
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
        area.x + prompt.len() as u16 + (input.visual_cursor().saturating_sub(scroll)) as u16,
        area.y,
    ));
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

fn panel<'a>(app: &App, title: &'a str) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(app.theme.accent()))
        .title(Span::styled(format!(" {title} "), app.theme.title()))
}

fn render_help(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let actions = keys::actions(app.view);
    let lines: Vec<Line> = actions
        .iter()
        .map(|(k, a)| {
            Line::from(vec![
                Span::styled(format!("{:>8}  ", k.display()), t.title()),
                Span::styled(a.label(), t.text()),
            ])
        })
        .collect();
    let rect = popup(area, 48, lines.len() as u16 + 2);
    frame.render_widget(Clear, rect);
    let title = format!("Keys · {}", app.view.label());
    frame.render_widget(Paragraph::new(lines).block(panel(app, &title)), rect);
}

fn render_palette(frame: &mut Frame, area: Rect, app: &App, p: &Palette) {
    let t = &app.theme;
    let matches = p.matches(app.view);
    let rect = popup(area, 56, 16);
    frame.render_widget(Clear, rect);
    let block = panel(app, "Commands");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let [input, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    render_input_line(frame, input, app, "› ", &p.input);
    let rows = list.height as usize;
    let start = p.cursor.saturating_sub(rows.saturating_sub(1));
    let lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, (k, a))| {
            let w = list.width as usize;
            let label = a.label();
            let key = k.display();
            let pad = w.saturating_sub(label.chars().count() + key.chars().count() + 2);
            let line = Line::from(vec![
                Span::styled(format!(" {label}"), t.text()),
                Span::raw(" ".repeat(pad)),
                Span::styled(format!("{key} "), t.dim()),
            ]);
            if i == p.cursor {
                line.style(t.selected())
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
}

fn render_info(frame: &mut Frame, area: Rect, app: &App, e: &Explanation) {
    let t = &app.theme;
    let item = &e.item;
    let field = |k: &'static str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<12}"), t.dim()),
            Span::styled(v, t.text()),
        ])
    };
    let mut lines = vec![
        Line::styled(app.ctx.tilde(&item.path), t.title()),
        Line::default(),
        field("size", item.size_human.clone()),
        Line::from(vec![
            Span::styled(format!("{:<12}", "bucket"), t.dim()),
            Span::styled(
                item.bucket_label.clone(),
                Style::new().fg(t.bucket(&app.ctx.rules, Some(&item.bucket))),
            ),
        ]),
        field(
            "rule",
            match (&item.rule_id, &item.rule_label) {
                (Some(id), Some(label)) => format!("{label} ({id})"),
                (Some(id), None) => id.clone(),
                _ => "none".into(),
            },
        ),
        field(
            "safety",
            format!(
                "{} {}",
                item.safety.glyph(),
                tildes(&app.ctx.home, &e.safety_reason)
            ),
        ),
        field("staleness", tildes(&app.ctx.home, &e.staleness)),
    ];
    if let Some(c) = &e.claimed_by
        && c != &item.path
    {
        lines.push(field("claimed by", app.ctx.tilde(c)));
    }
    if let Some(hint) = &item.regen_hint {
        lines.push(field("regen", hint.clone()));
    }
    let rect = popup(area, 84, lines.len() as u16 + 6);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel(app, "Why is this here")),
        rect,
    );
}

/// Shortens every home-directory path inside a sentence from core.
fn tildes(home: &std::path::Path, text: &str) -> String {
    text.replace(&format!("{}/", home.display()), "~/")
}

fn render_picker(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let [_, logo, tag, _, label, input, hint, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(splash::logo_height()),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(area);
    let w = splash::logo_width().min(logo.width);
    splash::render_logo(
        Rect {
            x: logo.x + (logo.width - w) / 2,
            width: w,
            ..logo
        },
        frame.buffer_mut(),
    );
    frame.render_widget(
        Paragraph::new(splash::TAGLINE).style(t.dim()).centered(),
        tag,
    );
    let field_w = 60.min(area.width.saturating_sub(4));
    let center = |r: Rect| Rect {
        x: r.x + (r.width - field_w) / 2,
        width: field_w,
        ..r
    };
    frame.render_widget(
        Paragraph::new("No index yet. Which folder should spacehog scan?").style(t.text()),
        center(label),
    );
    render_input_line(frame, center(input), app, "› ", &app.picker);
    let mut hints = vec![
        Span::styled("enter", t.title()),
        Span::styled(" scan   ", t.dim()),
        Span::styled("esc", t.title()),
        Span::styled(" quit", t.dim()),
    ];
    if let Some(msg) = &app.flash {
        hints.push(Span::styled(format!("   {msg}"), Style::new().fg(t.warn())));
    }
    frame.render_widget(Paragraph::new(Line::from(hints)), center(hint));
}
