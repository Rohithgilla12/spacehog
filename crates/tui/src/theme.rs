use ratatui::style::{Color, Modifier, Style};
use spacehog_core::rules::RuleSet;

/// Colours for one session. Bucket colours come from the rule set so every
/// view paints a bucket the same way.
#[derive(Debug, Clone)]
pub struct Theme {
    pub truecolour: bool,
    pub icons: bool,
}

const OTHER: &str = "#6272a4";

impl Theme {
    pub fn detect(icons: bool) -> Self {
        let truecolour = std::env::var("COLORTERM")
            .map(|v| v.contains("truecolor") || v.contains("24bit"))
            .unwrap_or(false);
        Self { truecolour, icons }
    }

    pub fn hex(&self, hex: &str) -> Color {
        let (r, g, b) = parse_hex(hex).unwrap_or((0x62, 0x72, 0xa4));
        if self.truecolour {
            Color::Rgb(r, g, b)
        } else {
            Color::Indexed(xterm_index(r, g, b))
        }
    }

    pub fn bucket(&self, rules: &RuleSet, bucket: Option<&str>) -> Color {
        let colour = bucket
            .and_then(|b| rules.bucket(b))
            .map_or(OTHER, |b| b.colour.as_str());
        self.hex(colour)
    }

    pub fn accent(&self) -> Color {
        self.hex("#ff79c6")
    }

    pub fn warn(&self) -> Color {
        self.hex("#ffb86c")
    }

    pub fn dim(&self) -> Style {
        Style::new().fg(self.hex("#7a7f98"))
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.hex("#e6e6f0"))
    }

    pub fn title(&self) -> Style {
        Style::new().fg(self.accent()).add_modifier(Modifier::BOLD)
    }

    pub fn selected(&self) -> Style {
        Style::new()
            .bg(self.hex("#383a4f"))
            .add_modifier(Modifier::BOLD)
    }

    pub fn border(&self) -> Style {
        Style::new().fg(self.hex("#44475a"))
    }

    /// A Nerd Font glyph when icons are on, otherwise the ASCII fallback.
    pub fn icon(&self, nerd: &'static str, ascii: &'static str) -> &'static str {
        if self.icons { nerd } else { ascii }
    }
}

pub fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// Nearest entry in the xterm 6x6x6 cube or greyscale ramp.
pub fn xterm_index(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| {
        STEPS
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| (i32::from(**s) - i32::from(v)).abs())
            .map_or(0, |(i, _)| i as u8)
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let cube_rgb = (STEPS[ri as usize], STEPS[gi as usize], STEPS[bi as usize]);
    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let grey_i = (avg.saturating_sub(8) / 10).min(23) as u8;
    let grey_v = 8 + 10 * grey_i;
    let dist = |(a, b2, c): (u8, u8, u8)| {
        let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
        d(a, r) + d(b2, g) + d(c, b)
    };
    if dist((grey_v, grey_v, grey_v)) < dist(cube_rgb) {
        232 + grey_i
    } else {
        cube
    }
}

/// Darkens a colour for unselected donut segments.
pub fn dim(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(r / 3, g / 3, b / 3),
        Color::Indexed(_) => Color::Indexed(238),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_maps_to_truecolour_or_xterm_cube() {
        let tc = Theme {
            truecolour: true,
            icons: false,
        };
        assert_eq!(tc.hex("#ff79c6"), Color::Rgb(255, 121, 198));
        let c256 = Theme {
            truecolour: false,
            icons: false,
        };
        assert_eq!(c256.hex("#ff0000"), Color::Indexed(196));
        assert_eq!(c256.hex("#000000"), Color::Indexed(16));
        assert_eq!(c256.hex("#808080"), Color::Indexed(244));
    }
}
