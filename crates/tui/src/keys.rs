use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Overview,
    Explorer,
    Treemap,
    Categories,
    Cleanup,
    Changes,
}

impl View {
    pub const ALL: [View; 6] = [
        View::Overview,
        View::Explorer,
        View::Treemap,
        View::Categories,
        View::Cleanup,
        View::Changes,
    ];

    pub fn label(self) -> &'static str {
        match self {
            View::Overview => "Overview",
            View::Explorer => "Explorer",
            View::Treemap => "Treemap",
            View::Categories => "Categories",
            View::Cleanup => "Cleanup",
            View::Changes => "Changes",
        }
    }

    /// Views later milestones fill in render a placeholder until then.
    pub fn is_built(self) -> bool {
        matches!(self, View::Overview | View::Explorer)
    }

    pub fn number(self) -> usize {
        View::ALL.iter().position(|v| *v == self).unwrap_or(0) + 1
    }

    pub fn next_built(self) -> View {
        let built: Vec<View> = View::ALL.into_iter().filter(|v| v.is_built()).collect();
        let i = built.iter().position(|v| *v == self).map_or(0, |i| i + 1);
        built[i % built.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Back,
    Help,
    Palette,
    Filter,
    Rescan,
    ToggleSize,
    ToggleStale,
    ToggleHidden,
    NextView,
    Show(View),
    Up,
    Down,
    Top,
    Bottom,
    PrevBucket,
    NextBucket,
    Open,
    Descend,
    Ascend,
    Queue,
    Reveal,
    CopyPath,
    Info,
    Preview,
}

impl Action {
    pub fn label(self) -> String {
        match self {
            Action::Quit => "Back, or quit from Overview".into(),
            Action::Back => "Back / close".into(),
            Action::Help => "Show keys".into(),
            Action::Palette => "Command palette".into(),
            Action::Filter => "Filter by name".into(),
            Action::Rescan => "Rescan this root".into(),
            Action::ToggleSize => "Toggle apparent / disk size".into(),
            Action::ToggleStale => "Toggle stale-only".into(),
            Action::ToggleHidden => "Toggle hidden files".into(),
            Action::NextView => "Next view".into(),
            Action::Show(v) => format!("Go to {}", v.label()),
            Action::Up => "Move up".into(),
            Action::Down => "Move down".into(),
            Action::Top => "Jump to top".into(),
            Action::Bottom => "Jump to bottom".into(),
            Action::PrevBucket => "Previous bucket".into(),
            Action::NextBucket => "Next bucket".into(),
            Action::Open => "Show in Explorer".into(),
            Action::Descend => "Descend into folder".into(),
            Action::Ascend => "Ascend to parent".into(),
            Action::Queue => "Add to cleanup queue".into(),
            Action::Reveal => "Open in file manager".into(),
            Action::CopyPath => "Copy path".into(),
            Action::Info => "Explain this item".into(),
            Action::Preview => "Toggle treemap preview".into(),
        }
    }
}

/// A key as the tables spell it: a code plus whether ctrl is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key(pub KeyCode, pub bool);

impl Key {
    const fn ch(c: char) -> Self {
        Key(KeyCode::Char(c), false)
    }

    const fn code(c: KeyCode) -> Self {
        Key(c, false)
    }

    pub fn from_event(ev: KeyEvent) -> Self {
        Key(ev.code, ev.modifiers.contains(KeyModifiers::CONTROL))
    }

    pub fn display(self) -> String {
        let base = match self.0 {
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Backspace => "bksp".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            other => format!("{other:?}"),
        };
        if self.1 { format!("ctrl+{base}") } else { base }
    }
}

pub type Keymap = &'static [(Key, Action)];

pub const GLOBAL: Keymap = &[
    (Key::ch('q'), Action::Quit),
    (Key(KeyCode::Char('c'), true), Action::Quit),
    (Key::code(KeyCode::Esc), Action::Back),
    (Key::ch('?'), Action::Help),
    (Key::ch(':'), Action::Palette),
    (Key::ch('/'), Action::Filter),
    (Key::ch('r'), Action::Rescan),
    (Key::ch('t'), Action::ToggleSize),
    (Key::ch('s'), Action::ToggleStale),
    (Key::ch('h'), Action::ToggleHidden),
    (Key::code(KeyCode::Tab), Action::NextView),
    (Key::ch('1'), Action::Show(View::Overview)),
    (Key::ch('2'), Action::Show(View::Explorer)),
    (Key::ch('3'), Action::Show(View::Treemap)),
    (Key::ch('4'), Action::Show(View::Categories)),
    (Key::ch('5'), Action::Show(View::Cleanup)),
    (Key::ch('6'), Action::Show(View::Changes)),
];

pub const OVERVIEW: Keymap = &[
    (Key::code(KeyCode::Up), Action::Up),
    (Key::ch('k'), Action::Up),
    (Key::code(KeyCode::Down), Action::Down),
    (Key::ch('j'), Action::Down),
    (Key::code(KeyCode::Left), Action::PrevBucket),
    (Key::code(KeyCode::Right), Action::NextBucket),
    (Key::code(KeyCode::Enter), Action::Open),
    (Key::ch('d'), Action::Queue),
    (Key::ch('i'), Action::Info),
    (Key::ch('g'), Action::Top),
    (Key::ch('G'), Action::Bottom),
];

pub const EXPLORER: Keymap = &[
    (Key::code(KeyCode::Up), Action::Up),
    (Key::ch('k'), Action::Up),
    (Key::code(KeyCode::Down), Action::Down),
    (Key::ch('j'), Action::Down),
    (Key::code(KeyCode::Enter), Action::Descend),
    (Key::code(KeyCode::Right), Action::Descend),
    (Key::ch('l'), Action::Descend),
    (Key::code(KeyCode::Left), Action::Ascend),
    (Key::code(KeyCode::Backspace), Action::Ascend),
    (Key::ch('g'), Action::Top),
    (Key::ch('G'), Action::Bottom),
    (Key::code(KeyCode::Home), Action::Top),
    (Key::code(KeyCode::End), Action::Bottom),
    (Key::ch('o'), Action::Reveal),
    (Key::ch('y'), Action::CopyPath),
    (Key::ch('i'), Action::Info),
    (Key::ch('p'), Action::Preview),
    (Key::ch('d'), Action::Queue),
];

pub fn keymap(view: View) -> Keymap {
    match view {
        View::Overview => OVERVIEW,
        View::Explorer => EXPLORER,
        _ => &[],
    }
}

/// View bindings shadow global ones, so `enter` means different things per view.
pub fn lookup(view: View, key: Key) -> Option<Action> {
    keymap(view)
        .iter()
        .chain(GLOBAL)
        .find(|(k, _)| *k == key)
        .map(|(_, a)| *a)
}

/// Every action reachable from `view`, each once, with its first key.
pub fn actions(view: View) -> Vec<(Key, Action)> {
    let mut out: Vec<(Key, Action)> = Vec::new();
    for (k, a) in keymap(view).iter().chain(GLOBAL) {
        if !out.iter().any(|(_, x)| x == a) {
            out.push((*k, *a));
        }
    }
    out
}

/// Subsequence match with a bonus for consecutive and word-start hits.
/// Returns None when `query` is not a subsequence of `text`.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut ti = 0;
    let mut prev: Option<usize> = None;
    for q in query.to_lowercase().chars() {
        let found = (ti..text.len()).find(|&i| text[i] == q)?;
        score += 1;
        if prev.is_some_and(|p| p + 1 == found) {
            score += 5;
        }
        if found == 0 || text[found - 1] == ' ' {
            score += 3;
        }
        prev = Some(found);
        ti = found + 1;
    }
    Some(score - text.len() as i32 / 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_binding_shadows_global() {
        assert_eq!(
            lookup(View::Explorer, Key(KeyCode::Enter, false)),
            Some(Action::Descend)
        );
        assert_eq!(
            lookup(View::Overview, Key(KeyCode::Enter, false)),
            Some(Action::Open)
        );
        assert_eq!(
            lookup(View::Explorer, Key(KeyCode::Char('q'), false)),
            Some(Action::Quit)
        );
    }

    #[test]
    fn fuzzy_prefers_word_starts_and_rejects_non_subsequences() {
        assert!(fuzzy_score("tgs", "Toggle stale-only").is_some());
        assert!(fuzzy_score("xyz", "Toggle stale-only").is_none());
        let a = fuzzy_score("res", "Rescan this root").unwrap();
        let b = fuzzy_score("res", "Toggle hidden files").unwrap_or(i32::MIN);
        assert!(a > b);
    }

    #[test]
    fn tab_cycles_only_built_views() {
        assert_eq!(View::Overview.next_built(), View::Explorer);
        assert_eq!(View::Explorer.next_built(), View::Overview);
        assert_eq!(View::Treemap.next_built(), View::Overview);
    }
}
