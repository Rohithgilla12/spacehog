pub mod app;
pub mod data;
pub mod explorer;
pub mod keys;
pub mod overview;
pub mod scanning;
pub mod splash;
pub mod sys;
pub mod theme;
pub mod ui;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};

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

const FRAME: Duration = Duration::from_millis(16);

pub fn run(opts: TuiOptions) -> anyhow::Result<()> {
    let ctx = Ctx::load()?;
    let mut app = App::new(ctx, Theme::detect(opts.icons), &opts)?;
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

/// Redraws only when state changed, at most once per frame.
fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    let mut dirty = true;
    let mut last_draw = Instant::now() - FRAME;
    while !app.quit {
        if dirty && last_draw.elapsed() >= FRAME {
            terminal.draw(|f| ui::draw(f, app))?;
            last_draw = Instant::now();
            dirty = false;
        }
        let wait = if dirty {
            FRAME.saturating_sub(last_draw.elapsed())
        } else {
            app.next_tick()
        };
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    app.handle_key(k)?;
                    dirty = true;
                }
                Event::Resize(..) => dirty = true,
                _ => {}
            }
        }
        if app.next_tick().is_zero() || app.scan.is_some() {
            dirty |= app.tick()?;
        }
    }
    Ok(())
}
