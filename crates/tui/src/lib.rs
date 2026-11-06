pub mod splash;

use std::path::PathBuf;

/// Options the `spacehog` binary passes when launching the TUI.
#[derive(Debug, Clone, Default)]
pub struct TuiOptions {
    /// Scan this root before showing the Overview.
    pub root: Option<PathBuf>,
    /// Open on the Changes view.
    pub changes: bool,
    pub icons: bool,
}

pub fn run(_opts: TuiOptions) -> anyhow::Result<()> {
    print!("{}", splash::splash_ansi(true));
    Ok(())
}
