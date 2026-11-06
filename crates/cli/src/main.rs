use std::io::IsTerminal;

fn main() {
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    print!("{}", spacehog_tui::splash::splash_ansi(color));
}
