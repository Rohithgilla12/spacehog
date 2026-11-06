pub mod app;
pub mod categories;
pub mod changes;
pub mod cleanup;
pub mod data;
pub mod explorer;
pub mod keys;
pub mod overview;
pub mod scanning;
pub mod splash;
pub mod sys;
pub mod theme;
pub mod treemap;
pub mod ui;
pub mod widgets;

use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};

pub use app::App;
pub use data::Ctx;
pub use theme::Theme;

/// Options the `spacehog` binary passes when launching the TUI.
#[derive(Debug, Clone, Default)]
pub struct TuiOptions {
    /// Scan this root before showing the Overview.
    pub root: Option<PathBuf>,
    /// Open on the Changes view.
    pub changes: bool,
    pub icons: bool,
}

pub fn run(opts: TuiOptions) -> anyhow::Result<()> {
    let ctx = Ctx::load()?;
    let theme = Theme::detect(&ctx.config, opts.icons);
    let mut app = App::new(ctx, theme, &opts)?;
    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = event_loop(&mut terminal, &mut app);
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    if app.cleanup.is_running() {
        eprintln!("spacehog: waiting for the running cleanup to finish…");
        app.cleanup.finish();
    }
    result
}

/// Redraws only when state changed, at most once per frame.
fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    let frame = app::FRAME;
    let mut dirty = true;
    let mut last_draw = Instant::now() - frame;
    while !app.quit {
        if dirty && last_draw.elapsed() >= frame {
            terminal.draw(|f| ui::draw(f, app))?;
            last_draw = Instant::now();
            dirty = false;
        }
        let wait = if dirty {
            frame.saturating_sub(last_draw.elapsed())
        } else {
            app.next_tick()
        };
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    app.handle_key(k)?;
                    dirty = true;
                }
                Event::Mouse(m) => {
                    app.handle_mouse(m)?;
                    dirty = true;
                }
                Event::Resize(..) => dirty = true,
                _ => {}
            }
        }
        if app.next_tick().is_zero() || app.animating() {
            dirty |= app.tick()?;
        }
    }
    Ok(())
}
