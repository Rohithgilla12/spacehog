use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;
use tui_big_text::{BigText, PixelSize};

pub const TAGLINE: &str = "Find the hog. Feed it to your agent.";

const GRADIENT: [(u8, u8, u8); 2] = [(255, 121, 198), (255, 184, 108)];

pub fn logo_width() -> u16 {
    "SPACEHOG".len() as u16 * 8
}

pub fn logo_height() -> u16 {
    4
}

pub fn render_logo(area: Rect, buf: &mut Buffer) {
    BigText::builder()
        .pixel_size(PixelSize::HalfHeight)
        .lines(vec!["SPACEHOG".into()])
        .build()
        .render(area, buf);
    let width = area.width.max(1) as f32;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let t = (x - area.left()) as f32 / width;
            let cell = &mut buf[(x, y)];
            cell.set_style(Style::new().fg(lerp(t)));
        }
    }
}

fn lerp(t: f32) -> Color {
    let (a, b) = (GRADIENT[0], GRADIENT[1]);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color::Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Renders the logo to an ANSI string for non-interactive output.
pub fn splash_ansi(color: bool) -> String {
    let area = Rect::new(0, 0, logo_width(), logo_height());
    let mut buf = Buffer::empty(area);
    render_logo(area, &mut buf);
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            match (color, cell.fg) {
                (true, Color::Rgb(r, g, b)) if cell.symbol() != " " => {
                    out.push_str(&format!("\x1b[38;2;{r};{g};{b}m{}\x1b[0m", cell.symbol()))
                }
                _ => out.push_str(cell.symbol()),
            }
        }
        out.push('\n');
    }
    out.push_str(TAGLINE);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_draws_glyphs_and_tagline() {
        let s = splash_ansi(false);
        assert!(s.lines().count() == 5, "four logo rows plus tagline");
        assert!(s.chars().any(|c| c != ' ' && c != '\n'), "logo has ink");
        assert!(s.ends_with(&format!("{TAGLINE}\n")));
    }
}
