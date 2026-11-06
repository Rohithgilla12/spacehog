use ratatui::style::{Color, Modifier, Style};
use spacehog_core::config::Config;
use spacehog_core::rules::RuleSet;

/// Which palette a session paints with. `Mono` is `NO_COLOR`: no colour at
/// all, so selection and emphasis fall back to reverse video and bold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Dark,
    Light,
    Mono,
}

/// Colours for one session. Bucket colours come from the rule set so every
/// view paints a bucket the same way.
#[derive(Debug, Clone)]
pub struct Theme {
    pub tone: Tone,
    pub truecolour: bool,
    pub icons: bool,
}

const OTHER: &str = "#6272a4";

struct Palette {
    text: &'static str,
    dim: &'static str,
    border: &'static str,
    accent: &'static str,
    warn: &'static str,
    good: &'static str,
    selected: &'static str,
    badge_fg: &'static str,
}

const DARK: Palette = Palette {
    text: "#e6e6f0",
    dim: "#7a7f98",
    border: "#44475a",
    accent: "#ff79c6",
    warn: "#ffb86c",
    good: "#50fa7b",
    selected: "#383a4f",
    badge_fg: "#282a36",
};

const LIGHT: Palette = Palette {
    text: "#1f2030",
    dim: "#6b6f80",
    border: "#c5c8d6",
    accent: "#c2185b",
    warn: "#b25e00",
    good: "#1b7f3b",
    selected: "#dfe3f0",
    badge_fg: "#ffffff",
};

/// The light theme paints its own background so it reads the same on any terminal.
pub const LIGHT_BG: &str = "#fafafc";

impl Theme {
    pub fn new(tone: Tone, truecolour: bool, icons: bool) -> Self {
        Self {
            tone,
            truecolour,
            icons,
        }
    }

    pub fn detect(config: &Config, icons: bool) -> Self {
        Self::resolve(
            config,
            icons,
            std::env::var_os("NO_COLOR").as_deref(),
            std::env::var("COLORTERM").ok().as_deref(),
        )
    }

    /// `NO_COLOR` (any non-empty value, per no-color.org) wins over `config.theme`.
    pub fn resolve(
        config: &Config,
        icons: bool,
        no_color: Option<&std::ffi::OsStr>,
        colorterm: Option<&str>,
    ) -> Self {
        let tone = if no_color.is_some_and(|v| !v.is_empty()) {
            Tone::Mono
        } else if config.theme.eq_ignore_ascii_case("light") {
            Tone::Light
        } else {
            Tone::Dark
        };
        let truecolour = colorterm.is_some_and(|v| v.contains("truecolor") || v.contains("24bit"));
        Self::new(tone, truecolour, icons)
    }

    pub fn mono(&self) -> bool {
        self.tone == Tone::Mono
    }

    fn palette(&self) -> &'static Palette {
        match self.tone {
            Tone::Light => &LIGHT,
            _ => &DARK,
        }
    }

    pub fn hex(&self, hex: &str) -> Color {
        self.rgb(parse_hex(hex).unwrap_or((0x62, 0x72, 0xa4)))
    }

    pub fn rgb(&self, (r, g, b): Rgb) -> Color {
        if self.mono() {
            Color::Reset
        } else if self.truecolour {
            Color::Rgb(r, g, b)
        } else {
            Color::Indexed(xterm_index(r, g, b))
        }
    }

    /// A bucket's colour as channels, darkened on the light theme so it reads on white.
    pub fn bucket_rgb(&self, hex: &str) -> Rgb {
        let c = parse_hex(hex).unwrap_or((0x62, 0x72, 0xa4));
        match self.tone {
            Tone::Light => scale(c, 0.72),
            _ => c,
        }
    }

    pub fn bucket_hex(&self, hex: &str) -> Color {
        self.rgb(self.bucket_rgb(hex))
    }

    pub fn bucket_of(&self, rules: &RuleSet, bucket: Option<&str>) -> Rgb {
        let colour = bucket
            .and_then(|b| rules.bucket(b))
            .map_or(OTHER, |b| b.colour.as_str());
        self.bucket_rgb(colour)
    }

    pub fn bucket(&self, rules: &RuleSet, bucket: Option<&str>) -> Color {
        self.rgb(self.bucket_of(rules, bucket))
    }

    /// One colour per safety class, shared by the stacked bar and its labels.
    pub fn safety_rgb(&self, s: spacehog_core::rules::Safety) -> Rgb {
        use spacehog_core::rules::Safety;
        self.bucket_rgb(match s {
            Safety::Regenerable => "#50fa7b",
            Safety::Cache => "#8be9fd",
            Safety::Review => "#ffb86c",
            Safety::Protected => "#6272a4",
        })
    }

    pub fn accent(&self) -> Color {
        self.hex(self.palette().accent)
    }

    pub fn warn(&self) -> Color {
        self.hex(self.palette().warn)
    }

    pub fn good(&self) -> Color {
        self.hex(self.palette().good)
    }

    pub fn dim(&self) -> Style {
        Style::new().fg(self.hex(self.palette().dim))
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.hex(self.palette().text))
    }

    pub fn title(&self) -> Style {
        Style::new().fg(self.accent()).add_modifier(Modifier::BOLD)
    }

    pub fn selected(&self) -> Style {
        if self.mono() {
            return Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        Style::new()
            .bg(self.hex(self.palette().selected))
            .add_modifier(Modifier::BOLD)
    }

    /// The status-strip pill for pending proposals.
    pub fn badge(&self) -> Style {
        if self.mono() {
            return Style::new().add_modifier(Modifier::REVERSED);
        }
        Style::new()
            .fg(self.hex(self.palette().badge_fg))
            .bg(self.accent())
    }

    pub fn border(&self) -> Style {
        Style::new().fg(self.hex(self.palette().border))
    }

    /// The frame background: only the light theme paints one.
    pub fn base(&self) -> Option<Style> {
        (self.tone == Tone::Light)
            .then(|| Style::new().bg(self.hex(LIGHT_BG)).fg(self.hex(LIGHT.text)))
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

pub type Rgb = (u8, u8, u8);

/// Scales brightness. Shading happens on channels, before the 256-colour
/// fallback, so darker tiles keep their hue on terminals without truecolour.
pub fn scale((r, g, b): Rgb, k: f32) -> Rgb {
    let s = |v: u8| (f32::from(v) * k).clamp(0.0, 255.0) as u8;
    (s(r), s(g), s(b))
}

/// Near-black or near-white, whichever reads better on `bg`.
pub fn ink_on((r, g, b): Rgb) -> Rgb {
    let lum = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if lum > 140.0 {
        (20, 20, 28)
    } else {
        (245, 245, 250)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_maps_to_truecolour_or_xterm_cube() {
        let tc = Theme::new(Tone::Dark, true, false);
        assert_eq!(tc.hex("#ff79c6"), Color::Rgb(255, 121, 198));
        let c256 = Theme::new(Tone::Dark, false, false);
        assert_eq!(c256.hex("#ff0000"), Color::Indexed(196));
        assert_eq!(c256.hex("#000000"), Color::Indexed(16));
        assert_eq!(c256.hex("#808080"), Color::Indexed(244));
    }

    #[test]
    fn mono_has_no_colours() {
        let m = Theme::new(Tone::Mono, true, false);
        assert_eq!(m.hex("#ff79c6"), Color::Reset);
        assert_eq!(m.selected().bg, None);
        assert!(m.selected().add_modifier.contains(Modifier::REVERSED));
    }
}
