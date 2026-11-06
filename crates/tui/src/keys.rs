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

    pub fn number(self) -> usize {
        View::ALL.iter().position(|v| *v == self).unwrap_or(0) + 1
    }

    pub fn next(self) -> View {
        View::ALL[self.number() % View::ALL.len()]
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
    Left,
    Right,
    ZoomIn,
    ZoomOut,
    DeeperMap,
    ShallowerMap,
    Expand,
    Collapse,
    Select,
    SelectAll,
    PrevTab,
    NextTab,
    Approve,
    Reject,
    Execute,
    ToggleMode,
    Undo,
    OlderBaseline,
    NewerBaseline,
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
            Action::Left => "Move left".into(),
            Action::Right => "Move right".into(),
            Action::ZoomIn => "Zoom into folder".into(),
            Action::ZoomOut => "Zoom out to parent".into(),
            Action::DeeperMap => "Show more levels".into(),
            Action::ShallowerMap => "Show fewer levels".into(),
            Action::Expand => "Expand or collapse rule".into(),
            Action::Collapse => "Collapse rule".into(),
            Action::Select => "Select item".into(),
            Action::SelectAll => "Select all visible".into(),
            Action::PrevTab => "Queue tab".into(),
            Action::NextTab => "Proposals tab".into(),
            Action::Approve => "Approve proposal".into(),
            Action::Reject => "Reject proposal, or drop from queue".into(),
            Action::Execute => "Clean up now".into(),
            Action::ToggleMode => "Toggle Trash / permanent".into(),
            Action::Undo => "Undo the last cleanup".into(),
            Action::OlderBaseline => "Compare with an older scan".into(),
            Action::NewerBaseline => "Compare with a newer scan".into(),
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

pub const TREEMAP: Keymap = &[
    (Key::code(KeyCode::Up), Action::Up),
    (Key::code(KeyCode::Down), Action::Down),
    (Key::code(KeyCode::Left), Action::Left),
    (Key::code(KeyCode::Right), Action::Right),
    (Key::code(KeyCode::Enter), Action::ZoomIn),
    (Key::code(KeyCode::Backspace), Action::ZoomOut),
    (Key::ch('+'), Action::DeeperMap),
    (Key::ch('='), Action::DeeperMap),
    (Key::ch('-'), Action::ShallowerMap),
    (Key::ch('d'), Action::Queue),
    (Key::ch('i'), Action::Info),
    (Key::ch('o'), Action::Reveal),
    (Key::ch('y'), Action::CopyPath),
];

pub const CATEGORIES: Keymap = &[
    (Key::code(KeyCode::Up), Action::Up),
    (Key::ch('k'), Action::Up),
    (Key::code(KeyCode::Down), Action::Down),
    (Key::ch('j'), Action::Down),
    (Key::code(KeyCode::Enter), Action::Expand),
    (Key::code(KeyCode::Right), Action::Expand),
    (Key::code(KeyCode::Left), Action::Collapse),
    (Key::ch(' '), Action::Select),
    (Key::ch('a'), Action::SelectAll),
    (Key::ch('d'), Action::Queue),
    (Key::ch('i'), Action::Info),
    (Key::ch('g'), Action::Top),
    (Key::ch('G'), Action::Bottom),
];

pub const CLEANUP: Keymap = &[
    (Key::code(KeyCode::Up), Action::Up),
    (Key::ch('k'), Action::Up),
    (Key::code(KeyCode::Down), Action::Down),
    (Key::ch('j'), Action::Down),
    (Key::code(KeyCode::Left), Action::PrevTab),
    (Key::code(KeyCode::Right), Action::NextTab),
    (Key::code(KeyCode::Enter), Action::Approve),
    (Key::ch('x'), Action::Reject),
    (Key::ch('D'), Action::Execute),
    (Key::ch('m'), Action::ToggleMode),
    (Key::ch('u'), Action::Undo),
    (Key::ch('i'), Action::Info),
];

pub const CHANGES: Keymap = &[
    (Key::code(KeyCode::Up), Action::Up),
    (Key::ch('k'), Action::Up),
    (Key::code(KeyCode::Down), Action::Down),
    (Key::ch('j'), Action::Down),
    (Key::ch('['), Action::OlderBaseline),
    (Key::ch(']'), Action::NewerBaseline),
];

pub fn keymap(view: View) -> Keymap {
    match view {
        View::Overview => OVERVIEW,
        View::Explorer => EXPLORER,
        View::Treemap => TREEMAP,
        View::Categories => CATEGORIES,
        View::Cleanup => CLEANUP,
        View::Changes => CHANGES,
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
    fn tab_cycles_every_view() {
        assert_eq!(View::Overview.next(), View::Explorer);
        assert_eq!(View::Cleanup.next(), View::Changes);
        assert_eq!(View::Changes.next(), View::Overview);
    }

    #[test]
    fn every_action_has_a_key_in_some_view() {
        let reachable: Vec<Action> = View::ALL
            .into_iter()
            .flat_map(|v| actions(v).into_iter().map(|(_, a)| a))
            .collect();
        for a in [
            Action::ZoomIn,
            Action::ZoomOut,
            Action::DeeperMap,
            Action::ShallowerMap,
            Action::Expand,
            Action::Collapse,
            Action::Select,
            Action::SelectAll,
            Action::PrevTab,
            Action::NextTab,
            Action::Approve,
            Action::Reject,
            Action::Execute,
            Action::ToggleMode,
            Action::Undo,
            Action::OlderBaseline,
            Action::NewerBaseline,
        ] {
            assert!(reachable.contains(&a), "{a:?} has no key");
        }
    }
}
