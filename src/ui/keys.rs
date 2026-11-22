use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Quit,
    Help,
    Settings,
    Players,
    Syncs,
    Library,
    Updates,
    Refetch,
    ToggleStyle,
    ToggleRenderer,
    ToggleTranslation,
    ToggleRomanization,
    ToggleBackgroundVocals,
    ToggleHeader,
    Scroll(f64),
    Page(f64),
    ScrollTop,
    ScrollBottom,
    Follow,
    Offset(i64),
    OffsetReset,
    PlayPause,
    Seek(i8),
    Next,
    Previous,
    SeekToCursor,
}

pub struct Binding {
    pub keys: &'static str,
    pub desc: &'static str,
}

pub struct BindingGroup {
    pub title: &'static str,
    pub bindings: &'static [Binding],
}

pub const HELP: &[BindingGroup] = &[
    BindingGroup {
        title: "General",
        bindings: &[
            Binding { keys: "?", desc: "Show or hide this help" },
            Binding { keys: "Esc", desc: "Settings" },
            Binding { keys: "p", desc: "Choose the player to follow" },
            Binding { keys: "s", desc: "Search all providers and pick another sync" },
            Binding { keys: "L", desc: "Local library: add your own lyrics file, URL or clipboard" },
            Binding { keys: "R", desc: "Fetch lyrics again, skipping the cache" },
            Binding { keys: "U", desc: "Updates: check for, install and restart into a new version" },
            Binding { keys: "Tab S-Tab", desc: "In dialogs: jump to the next / previous category" },
            Binding { keys: "q  Ctrl-C", desc: "Quit" },
            Binding { keys: "Mouse", desc: "Click to select, again to open; wheel moves; click outside closes" },
        ],
    },
    BindingGroup {
        title: "View",
        bindings: &[
            Binding { keys: "v", desc: "Switch style (Spring / Bloom)" },
            Binding { keys: "m", desc: "Switch renderer (cell / pixel)" },
            Binding { keys: "t", desc: "Toggle translation" },
            Binding { keys: "r", desc: "Toggle romanization" },
            Binding { keys: "b", desc: "Toggle background vocals" },
            Binding { keys: "i", desc: "Toggle the track header" },
        ],
    },
    BindingGroup {
        title: "Scrolling",
        bindings: &[
            Binding { keys: "j k  ↓ ↑", desc: "Scroll by a line (pauses auto-follow)" },
            Binding { keys: "PgDn PgUp", desc: "Scroll by half a screen" },
            Binding { keys: "g G", desc: "Jump to the top / bottom" },
            Binding { keys: "f", desc: "Follow the song again" },
        ],
    },
    BindingGroup {
        title: "Timing",
        bindings: &[
            Binding { keys: "[ ]", desc: "Shift lyrics for this track by ∓50 ms" },
            Binding { keys: "{ }", desc: "Shift lyrics for this track by ∓250 ms" },
            Binding { keys: "=", desc: "Reset this track's offset" },
        ],
    },
    BindingGroup {
        title: "Playback (enable in Settings → Controls)",
        bindings: &[
            Binding { keys: "Space", desc: "Play / pause" },
            Binding { keys: "h l  ← →", desc: "Seek backward / forward" },
            Binding { keys: "n N", desc: "Next / previous track" },
            Binding { keys: "Enter", desc: "Jump to the line under the cursor while scrolling" },
            Binding { keys: "Click bar", desc: "Seek; drag along the progress bar to scrub" },
            Binding { keys: "Click lyric", desc: "Jump to that line or word (Settings → Scrolling)" },
        ],
    },
];

pub fn map(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('?') | KeyCode::F(1) => Action::Help,
        KeyCode::Esc | KeyCode::F(2) => Action::Settings,
        KeyCode::Char('p') => Action::Players,
        KeyCode::Char('s') => Action::Syncs,
        KeyCode::Char('L') | KeyCode::F(3) => Action::Library,
        KeyCode::Char('R') | KeyCode::F(5) => Action::Refetch,
        KeyCode::Char('U') => Action::Updates,
        KeyCode::Char('v') => Action::ToggleStyle,
        KeyCode::Char('m') => Action::ToggleRenderer,
        KeyCode::Char('t') => Action::ToggleTranslation,
        KeyCode::Char('r') => Action::ToggleRomanization,
        KeyCode::Char('b') => Action::ToggleBackgroundVocals,
        KeyCode::Char('i') => Action::ToggleHeader,
        KeyCode::Char('j') | KeyCode::Down => Action::Scroll(1.0),
        KeyCode::Char('k') | KeyCode::Up => Action::Scroll(-1.0),
        KeyCode::PageDown | KeyCode::Char('d') if ctrl || key.code == KeyCode::PageDown => Action::Page(0.5),
        KeyCode::PageUp | KeyCode::Char('u') if ctrl || key.code == KeyCode::PageUp => Action::Page(-0.5),
        KeyCode::Char('g') | KeyCode::Home => Action::ScrollTop,
        KeyCode::Char('G') | KeyCode::End => Action::ScrollBottom,
        KeyCode::Char('f') => Action::Follow,
        KeyCode::Char('[') => Action::Offset(-50),
        KeyCode::Char(']') => Action::Offset(50),
        KeyCode::Char('{') => Action::Offset(-250),
        KeyCode::Char('}') => Action::Offset(250),
        KeyCode::Char('=') => Action::OffsetReset,
        KeyCode::Char(' ') => Action::PlayPause,
        KeyCode::Char('h') | KeyCode::Left => Action::Seek(-1),
        KeyCode::Char('l') | KeyCode::Right => Action::Seek(1),
        KeyCode::Char('n') => Action::Next,
        KeyCode::Char('N') => Action::Previous,
        KeyCode::Enter => Action::SeekToCursor,
        _ => return None,
    })
}
