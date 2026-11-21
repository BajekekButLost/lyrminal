use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::anim::Rgb;
use crate::app::{App, LyricsState};
use crate::config::{Config, PlayerMode, Preset, VisualStyle};
use crate::lyrics::SyncKind;
use crate::providers::{ordered_ids, provider_name};
use crate::theme::Theme;
use crate::ui::draw::{Target, UiHits, panel, panel_ink, row_rect, spinner, truncate};
use crate::ui::settings::{self, Item, Kind};

pub const ADVANCE_AFTER: Duration = Duration::from_millis(1800);
const WIDE_FROM: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Player,
    Providers,
    Library,
    Song,
    Customize,
    Done,
}

impl Step {
    pub const ALL: [Step; 6] = [Step::Player, Step::Providers, Step::Library, Step::Song, Step::Customize, Step::Done];

    pub fn title(self) -> &'static str {
        match self {
            Step::Player => "Your music player",
            Step::Providers => "Lyrics providers",
            Step::Library => "Your own syncs",
            Step::Song => "Play a song",
            Step::Customize => "Make it yours",
            Step::Done => "All set",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn next(self) -> Option<Step> {
        Self::ALL.get(self.index() + 1).copied()
    }

    pub fn prev(self) -> Option<Step> {
        self.index().checked_sub(1).map(|i| Self::ALL[i])
    }
}

pub enum Row {
    Auto,
    Player(usize),
    Setting(Item),
    Provider(String),
    Answer(bool),
    Next,
    Back,
    Skip,
}

impl Row {
    pub fn kind(&self) -> Option<&Kind> {
        match self {
            Row::Setting(it) => Some(&it.kind),
            _ => None,
        }
    }

    pub fn is_folder(&self) -> bool {
        matches!(self.kind(), Some(Kind::Text { folder: true, .. }))
    }

    pub fn adjustable(&self) -> bool {
        matches!(self, Row::Provider(_)) || self.kind().is_some_and(settings::adjustable)
    }

    pub fn is_text(&self) -> bool {
        matches!(self.kind(), Some(Kind::Text { .. }))
    }
}

pub struct Onboarding {
    pub step: Step,
    pub selected: usize,
    pub editing: Option<String>,
    pub found_at: Option<Instant>,
    pub auto_advance: bool,
    pub stores_syncs: Option<bool>,
}

impl Onboarding {
    pub fn new() -> Self {
        Self {
            step: Step::Player,
            selected: 0,
            editing: None,
            found_at: None,
            auto_advance: false,
            stores_syncs: None,
        }
    }

    pub fn go(&mut self, step: Step) {
        self.auto_advance = step == Step::Song && step.index() > self.step.index();
        self.step = step;
        self.selected = 0;
        self.editing = None;
        self.found_at = None;
    }

    pub fn default_row(&self, rows: &[Row]) -> usize {
        if self.step == Step::Customize || (self.step == Step::Library && self.stores_syncs.is_none()) {
            return 0;
        }
        rows.iter().position(|r| matches!(r, Row::Next)).unwrap_or(0)
    }

    pub fn rows(&self, app: &App) -> Vec<Row> {
        let cfg = &app.cfg;
        let setting = |section: &str, label: &str| settings::find_item(cfg, section, label).map(Row::Setting);
        let mut rows = Vec::new();
        match self.step {
            Step::Player => {
                rows.push(Row::Auto);
                rows.extend((0..app.players.len()).map(Row::Player));
                rows.extend(setting("Controls", "Playback controls"));
                rows.extend([Row::Next, Row::Skip]);
            }
            Step::Providers => {
                rows.extend(ordered_ids(cfg).into_iter().map(Row::Provider));
                if let Some(mut key) = settings::provider_page(crate::providers::spicy::ID)
                    .into_iter()
                    .find(|i| matches!(i.kind, Kind::Text { .. }))
                {
                    key.label = "Spicy Lyrics API key".into();
                    key.desc = format!("Optional. {} Without one, Spicy Lyrics is skipped and the other providers are used.", key.desc);
                    rows.push(Row::Setting(key));
                }
                rows.extend([Row::Next, Row::Back, Row::Skip]);
            }
            Step::Library => {
                rows.extend([Row::Answer(true), Row::Answer(false)]);
                if self.stores_syncs == Some(true) {
                    let mut folder = settings::default_library_item();
                    folder.label = "Library location".into();
                    folder.desc = format!("Optional. {}", folder.desc);
                    rows.push(Row::Setting(folder));
                }
                rows.extend([Row::Next, Row::Back, Row::Skip]);
            }
            Step::Song => rows.extend([Row::Next, Row::Back, Row::Skip]),
            Step::Customize => {
                rows.extend(setting("Appearance", "Preset"));
                if let Some(Row::Setting(mut r)) = setting("Appearance", "Renderer") {
                    if !app.pixel_supported() {
                        r.desc = "This terminal has no kitty graphics support, so only the cell renderer works here. kitty, Ghostty and WezTerm can draw the pixel renderer with real fonts and blur.".into();
                    }
                    rows.push(Row::Setting(r));
                }
                for label in ["Style", "Background", "Album art theming", "Alignment", "Fade sung lines"] {
                    rows.extend(setting("Appearance", label));
                }
                if app.use_pixel() {
                    rows.extend(setting("Appearance", "Pixel font size"));
                    if cfg.appearance.style == VisualStyle::Spring {
                        rows.extend(setting("Appearance", "Syllable spacing"));
                    }
                } else {
                    rows.extend(setting("Appearance", "Line gap"));
                }
                rows.extend(setting("Scrolling", "Animation"));
                if let LyricsState::Ready { prepared, .. } = &app.lyrics {
                    if prepared.lyrics.has_translation() {
                        rows.extend(setting("Lyrics", "Translation"));
                    }
                    if prepared.lyrics.has_romanization() {
                        rows.extend(setting("Lyrics", "Romanization"));
                    }
                }
                rows.extend([Row::Next, Row::Back]);
            }
            Step::Done => rows.extend([Row::Next, Row::Back]),
        }
        rows
    }
}

pub struct Split {
    pub panel: Rect,
    pub main: Rect,
    pub side: bool,
}

pub fn split(area: Rect) -> Split {
    if area.width >= WIDE_FROM {
        let w = (area.width * 2 / 5).clamp(44, 60);
        return Split {
            panel: Rect { width: w, ..area },
            main: Rect {
                x: area.x + w,
                width: area.width - w,
                ..area
            },
            side: true,
        };
    }
    let h = (area.height * 9 / 20)
        .clamp(10, 18)
        .min(area.height.saturating_sub(8))
        .max(area.height.min(6));
    Split {
        panel: Rect { height: h, ..area },
        main: Rect {
            y: area.y + h,
            height: area.height - h,
            ..area
        },
        side: false,
    }
}

fn lyrics_ready(app: &App) -> bool {
    matches!(app.lyrics, LyricsState::Ready { .. })
}

fn player_name(app: &App) -> String {
    match (&app.snapshot, app.cfg.player.mode) {
        (Some(s), _) => s.player.display_name(),
        (None, PlayerMode::Fixed) if !app.cfg.player.target.is_empty() => app.cfg.player.target.clone(),
        _ => "your music player".into(),
    }
}

fn row_label(row: &Row, step: Step, app: &App) -> String {
    match row {
        Row::Auto => "Auto · whichever is playing".into(),
        Row::Player(i) => app
            .players
            .get(*i)
            .map(|p| format!("{} {}", p.status.icon(), p.display_name()))
            .unwrap_or_default(),
        Row::Setting(it) => it.label.clone(),
        Row::Provider(id) => provider_name(id).to_string(),
        Row::Answer(true) => "Yes, I keep sync files".into(),
        Row::Answer(false) => "No, I don't".into(),
        Row::Next => match step {
            Step::Song if !lyrics_ready(app) => "Skip this step".into(),
            Step::Customize => "Finish".into(),
            Step::Done => "Start listening".into(),
            _ => "Continue".into(),
        },
        Row::Back => "Back".into(),
        Row::Skip => "Skip the tour".into(),
    }
}

fn row_value(row: &Row, app: &App, editing: Option<&str>) -> String {
    match row {
        Row::Auto => if app.cfg.player.mode == PlayerMode::Auto { "✓" } else { "" }.into(),
        Row::Player(i) => {
            let Some(p) = app.players.get(*i) else { return String::new() };
            let following = app.snapshot.as_ref().is_some_and(|s| s.player.bus == p.bus);
            let fixed = app.cfg.player.mode == PlayerMode::Fixed && p.matches(&app.cfg.player.target);
            match (fixed, following) {
                (true, _) => "✓ only this one".into(),
                (false, true) => "following".into(),
                _ => String::new(),
            }
        }
        Row::Setting(it) => match (editing, &it.kind) {
            (Some(buf), Kind::Text { secret: true, .. }) => format!("{}▏", "•".repeat(buf.chars().count())),
            (Some(buf), _) => format!("{buf}▏"),
            _ => settings::value_text(&app.cfg, &it.kind),
        },
        Row::Provider(id) => settings::value_text(&app.cfg, &Kind::Provider(id.clone())),
        Row::Answer(b) => if app.guide.as_ref().and_then(|g| g.stores_syncs) == Some(*b) { "✓" } else { "" }.into(),
        Row::Next | Row::Back | Row::Skip => String::new(),
    }
}

fn spicy_waiting(cfg: &Config) -> bool {
    cfg.providers.is_enabled(crate::providers::spicy::ID) && cfg.providers.spicy_lyrics.effective_key().is_none()
}

fn row_desc(row: &Row, step: Step, app: &App) -> String {
    let cfg = &app.cfg;
    match row {
        Row::Auto => format!(
            "Follow whichever player is playing, preferring {}. Best if you switch between players.",
            if cfg.player.preferred.is_empty() { "none in particular".to_string() } else { cfg.player.preferred.join(", ") }
        ),
        Row::Player(i) => app
            .players
            .get(*i)
            .map(|p| {
                let process = p.process.clone().unwrap_or_else(|| p.short_bus().to_string());
                format!("Only follow {} ({process}), even while another player is playing.", p.display_name())
            })
            .unwrap_or_default(),
        Row::Setting(it) => it.desc.clone(),
        Row::Provider(id) => {
            let mut s = format!(
                "Space or Enter turns {} on or off. K and J, or Shift+↑↓, move it: lyrics come from the first provider that has them.",
                provider_name(id)
            );
            if id == crate::providers::local::ID {
                s.push_str(" Add your own lyrics to it any time with L.");
            }
            if id == crate::providers::spicy::ID {
                s.push_str(" It needs a free API key, set below.");
            }
            s
        }
        Row::Answer(true) => "Pick the folder where your sync files live, so importing one with L starts right there.".into(),
        Row::Answer(false) => "Go on. You can still add lyrics any time with L, and set a folder later in Settings → Providers → Local library.".into(),
        Row::Next => match step {
            Step::Player if app.snapshot.is_none() => "No player is followed yet. You can go on and choose one later with p.".into(),
            Step::Player => "Go on to the lyrics providers.".into(),
            Step::Providers if ordered_ids(cfg).iter().all(|id| !cfg.providers.is_enabled(id)) => {
                "Every provider is off, so no lyrics can be found. Turn one on first.".into()
            }
            Step::Providers if spicy_waiting(cfg) => {
                "Spicy Lyrics has no API key yet, so it is skipped until you add one. The other providers still work.".into()
            }
            Step::Providers => "Go on to your own syncs.".into(),
            Step::Library => "Go on and play a song.".into(),
            Step::Song if lyrics_ready(app) => "Go on and try out the looks.".into(),
            Step::Song => "Go on without lyrics. The looks are easier to judge with a song that has them.".into(),
            Step::Customize => "Keep these settings and see the keys worth knowing.".into(),
            Step::Done => "Close the tour. Esc opens every setting, and Settings → About runs this tour again.".into(),
        },
        Row::Back => "Go back to the previous step.".into(),
        Row::Skip => "Close the tour now. Settings → About → Welcome tour brings it back.".into(),
    }
}

enum Tone {
    Good,
    Wait,
    Bad,
    Note,
}

fn status(step: Step, app: &App, found_at: Option<Instant>) -> Vec<(Tone, String)> {
    let cfg = &app.cfg;
    let mut out = Vec::new();
    let spin = spinner(app);
    match step {
        Step::Player => match &app.snapshot {
            Some(s) => {
                out.push((Tone::Good, format!("✓ Following {}", s.player.display_name())));
                if let Some(t) = &s.track {
                    out.push((Tone::Note, format!("{} {} — {}", s.status.icon(), t.title, t.artists.join(", "))));
                }
            }
            None => {
                if cfg.player.mode == PlayerMode::Fixed && !app.players.is_empty() {
                    out.push((Tone::Bad, format!("✗ No running player matches “{}”", cfg.player.target)));
                } else if app.players.is_empty() {
                    out.push((Tone::Wait, format!("{spin} Looking for a music player…")));
                } else {
                    out.push((Tone::Wait, format!("{spin} Waiting for a player to start playing…")));
                }
                if let Some(e) = &app.player_error {
                    out.push((Tone::Note, e.clone()));
                }
            }
        },
        Step::Providers => {
            let on = ordered_ids(cfg).into_iter().filter(|id| cfg.providers.is_enabled(id)).count();
            if on == 0 {
                out.push((Tone::Bad, "✗ Every provider is off".into()));
            } else if spicy_waiting(cfg) {
                out.push((Tone::Bad, "Spicy Lyrics is waiting for an API key".into()));
            } else {
                out.push((Tone::Good, format!("✓ {on} provider{} ready", if on == 1 { "" } else { "s" })));
            }
        }
        Step::Library => {
            let dir = cfg.providers.local_library.default_dir.trim();
            match app.guide.as_ref().and_then(|g| g.stores_syncs) {
                Some(true) if dir.is_empty() => out.push((Tone::Note, "Setting a folder is optional; you can add one later in Settings → Providers → Local library.".into())),
                Some(true) if cfg.providers.local_library.import_start().is_none() => out.push((Tone::Bad, format!("✗ {dir} is not a folder"))),
                Some(true) => out.push((Tone::Good, format!("✓ Imports start in {dir}"))),
                Some(false) => out.push((Tone::Note, "No problem. L still adds lyrics whenever you need them.".into())),
                None => {}
            }
        }
        Step::Song => {
            let title = app.track.as_ref().map(|t| t.title.clone()).unwrap_or_default();
            match &app.lyrics {
                LyricsState::NoPlayer if app.snapshot.is_none() => {
                    out.push((Tone::Wait, format!("{spin} Waiting for {}…", player_name(app))));
                }
                LyricsState::NoPlayer | LyricsState::NoTrack => {
                    out.push((Tone::Wait, format!("{spin} Waiting for a song in {}…", player_name(app))));
                }
                LyricsState::Unsupported(what) => out.push((Tone::Bad, format!("✗ {what}: there are no lyrics for this, play a song"))),
                LyricsState::Waiting | LyricsState::Loading => out.push((Tone::Wait, format!("{spin} Looking for lyrics for “{title}”…"))),
                LyricsState::NotFound(_) => {
                    out.push((Tone::Bad, format!("✗ No lyrics found for “{title}”")));
                    out.push((Tone::Note, "Try another song, press s to search every provider, or L to add your own lyrics.".into()));
                }
                LyricsState::Ready { resolution, prepared, .. } => {
                    out.push((
                        Tone::Good,
                        format!("✓ {} from {}", prepared.lyrics.kind.label(), provider_name(&resolution.candidate.provider)),
                    ));
                    if prepared.lyrics.kind == SyncKind::Static {
                        out.push((Tone::Note, "These lyrics are not synced. A synced song shows the animations best, but you can go on.".into()));
                    } else if found_at.is_some() {
                        out.push((Tone::Note, format!("{spin} On to the looks in a moment…")));
                    }
                }
            }
        }
        Step::Customize => {
            if !lyrics_ready(app) {
                out.push((Tone::Bad, "Play a song with lyrics to see the changes.".into()));
            } else if Preset::detect(cfg) != Some(Preset::SpicyLyrics) {
                out.push((Tone::Note, "Tip: the Spicy Lyrics preset switches everything to the Spicy Lyrics look at once.".into()));
            }
        }
        Step::Done => {}
    }
    out
}

fn intro(step: Step, app: &App, side: bool) -> String {
    let place = if side { "on the right" } else { "below" };
    match step {
        Step::Player => "Lyrminal shows synced lyrics for whatever your music player is playing. It finds players through MPRIS, so open Spotify, spotifast, a browser or any other MPRIS player and it shows up here.".into(),
        Step::Providers => "Lyrics come from the first provider in this list that has them. lrc.red and LRCLIB work without any setup.".into(),
        Step::Library => "Lyrminal can also use sync files you keep yourself, such as LRC, TTML or SRT, from its local library. Do you store syncs for your songs?".into(),
        Step::Song => format!("Play a song in {}. Lyrminal looks it up and the lyrics appear {place}.", player_name(app)),
        Step::Customize => format!("Try the main looks while the song plays; the lyrics {place} change as you go. ←→ changes the selected setting."),
        Step::Done => "You're ready. These keys are worth knowing, and ? lists every one.".into(),
    }
}

const DONE_KEYS: [(&str, &str); 8] = [
    ("Esc", "every setting"),
    ("?", "all the keys"),
    ("s", "choose another sync"),
    ("L", "add your own lyrics"),
    ("v", "switch the style"),
    ("m", "switch the renderer"),
    ("p", "choose the player"),
    ("[ ]", "nudge the timing"),
];

pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0;
    for word in text.split_whitespace() {
        let ww = UnicodeWidthStr::width(word);
        if cur_w > 0 && cur_w + 1 + ww > width {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if ww > width {
            for ch in word.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if cur_w + cw > width {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push(ch);
                cur_w += cw;
            }
            continue;
        }
        if cur_w > 0 {
            cur.push(' ');
            cur_w += 1;
        }
        cur.push_str(word);
        cur_w += ww;
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

fn strong(theme: &Theme, bg: Rgb) -> Style {
    Style::default().fg(theme.accent.to_color()).bg(bg.to_color()).add_modifier(Modifier::BOLD)
}

pub fn draw(f: &mut Frame, app: &App, area: Rect, hits: &mut UiHits) {
    let Some(guide) = &app.guide else { return };
    let theme = &app.theme;
    let side = split(f.area()).side;
    let (bg, style) = panel(theme);
    hits.guide = Some(area);
    let index = guide.step.index();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent.to_color()).bg(bg.to_color()))
        .title(Span::styled(" Welcome to Lyrminal ", strong(theme, bg)))
        .title_bottom(Line::from(Span::styled(format!(" {} / {} ", index + 1, Step::ALL.len()), panel_ink(theme, bg, 0.5))).right_aligned())
        .style(style);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    if inner.width < 8 || inner.height < 4 {
        return;
    }
    let rows = guide.rows(app);
    let selected = guide.selected.min(rows.len().saturating_sub(1));
    let folder = rows.get(selected).is_some_and(Row::is_folder);
    let desc_text = if guide.editing.is_some() && folder {
        "Type or paste a folder, Tab to complete it, Enter to save, Esc to cancel, Ctrl-U to clear.".to_string()
    } else if guide.editing.is_some() {
        "Type or paste the key, Enter to save, Esc to cancel, Ctrl-U to clear.".to_string()
    } else {
        rows.get(selected).map(|r| row_desc(r, guide.step, app)).unwrap_or_default()
    };
    let intro_text = intro(guide.step, app, side);
    let columns = !side && inner.width >= 72;
    let footer = Rect {
        y: inner.y + inner.height - 1,
        height: 1,
        ..inner
    };
    let content = Rect {
        height: inner.height - 1,
        ..inner
    };
    let (body, text, rule) = if columns {
        let lw = content.width * 11 / 20;
        (
            Rect { width: lw, ..content },
            Rect {
                x: content.x + lw + 3,
                width: content.width - lw - 3,
                ..content
            },
            Rect {
                x: content.x + lw + 1,
                width: 1,
                ..content
            },
        )
    } else {
        let most = (content.height / 3).max(2) as usize;
        let n = wrap(&desc_text, content.width as usize).len().clamp(2, most) as u16;
        let body_h = content.height.saturating_sub(n + 1);
        (
            Rect { height: body_h, ..content },
            Rect {
                y: content.y + body_h + 1,
                height: n,
                ..content
            },
            Rect {
                y: content.y + body_h,
                height: 1,
                ..content
            },
        )
    };
    let width = body.width as usize;
    let mut display: Vec<(Option<usize>, Line)> = Vec::new();
    let dots: String = Step::ALL
        .iter()
        .enumerate()
        .map(|(i, _)| if i == index { "●" } else if i < index { "•" } else { "○" })
        .collect::<Vec<_>>()
        .join(" ");
    display.push((
        None,
        Line::from(vec![
            Span::styled(format!("{dots}  "), Style::default().fg(theme.accent.to_color()).bg(bg.to_color())),
            Span::styled(guide.step.title(), panel_ink(theme, bg, 0.95).add_modifier(Modifier::BOLD)),
        ]),
    ));
    if !columns {
        display.push((None, Line::default()));
        for l in wrap(&intro_text, width) {
            display.push((None, Line::from(Span::styled(l, panel_ink(theme, bg, 0.75)))));
        }
    }
    if guide.step == Step::Done {
        display.push((None, Line::default()));
        for (k, d) in DONE_KEYS {
            display.push((
                None,
                Line::from(vec![
                    Span::styled(format!("  {k:<6}"), panel_ink(theme, bg, 0.95).add_modifier(Modifier::BOLD)),
                    Span::styled(d, panel_ink(theme, bg, 0.65)),
                ]),
            ));
        }
    }
    let st = status(guide.step, app, guide.found_at);
    if !st.is_empty() {
        display.push((None, Line::default()));
    }
    for (tone, text) in st {
        let s = match tone {
            Tone::Good => strong(theme, bg),
            Tone::Wait => panel_ink(theme, bg, 0.8),
            Tone::Bad => panel_ink(theme, bg, 0.9).add_modifier(Modifier::BOLD),
            Tone::Note => panel_ink(theme, bg, 0.55),
        };
        for l in wrap(&text, width) {
            display.push((None, Line::from(Span::styled(l, s))));
        }
    }
    display.push((None, Line::default()));
    let sel_bg = bg.mix(theme.accent, 0.18);
    let mut nav_started = false;
    for (i, row) in rows.iter().enumerate() {
        let nav = matches!(row, Row::Next | Row::Back | Row::Skip);
        if nav && !nav_started && i > 0 {
            display.push((None, Line::default()));
        }
        nav_started |= nav;
        let is_sel = i == selected;
        let row_bg = if is_sel { sel_bg } else { bg };
        let editing = guide.editing.as_deref().filter(|_| is_sel);
        let label = row_label(row, guide.step, app);
        let mut value = row_value(row, app, editing);
        if is_sel && editing.is_none() && row.adjustable() {
            value = format!("‹ {value} ›");
        }
        let room = width.saturating_sub(2);
        let value = if value.ends_with('▏') {
            let keep = room.saturating_sub(4).max(1);
            let chars: Vec<char> = value.chars().collect();
            chars[chars.len().saturating_sub(keep)..].iter().collect()
        } else {
            truncate(&value, room.saturating_sub(4))
        };
        let vw = UnicodeWidthStr::width(value.as_str());
        let label = truncate(&label, room.saturating_sub(vw + 1));
        let lw = UnicodeWidthStr::width(label.as_str());
        let fill = room.saturating_sub(lw + vw);
        let label_style = match row {
            Row::Next => strong(theme, row_bg),
            Row::Back | Row::Skip => panel_ink(theme, row_bg, if is_sel { 0.9 } else { 0.55 }),
            _ => panel_ink(theme, row_bg, if is_sel { 1.0 } else { 0.8 }),
        };
        let value_style = if is_sel { strong(theme, row_bg) } else { panel_ink(theme, row_bg, 0.6) };
        display.push((
            Some(i),
            Line::from(vec![
                Span::styled(if is_sel { "› " } else { "  " }, Style::default().fg(theme.accent.to_color()).bg(row_bg.to_color())),
                Span::styled(label, label_style),
                Span::styled(" ".repeat(fill), panel_ink(theme, row_bg, 0.5)),
                Span::styled(value, value_style),
            ]),
        ));
    }

    let visible = body.height as usize;
    let sel_pos = display.iter().position(|(r, _)| *r == Some(selected)).unwrap_or(0);
    let scroll = (sel_pos + 2).saturating_sub(visible).min(display.len().saturating_sub(visible));
    let mut lines = Vec::new();
    for (n, (row, line)) in display.into_iter().enumerate().skip(scroll).take(visible) {
        let y = body.y + (n - scroll) as u16;
        if let Some(i) = row {
            hits.push(row_rect(body.x, y, body.width), Target::Row(i), body);
            if i == selected && guide.editing.is_none() && rows[i].adjustable() {
                let vw = line.spans.get(3).map(|s| UnicodeWidthStr::width(s.content.as_ref())).unwrap_or(0) as u16;
                let vx = body.x + body.width.saturating_sub(vw);
                hits.push(row_rect(vx, y, 2), Target::Decrease, body);
                hits.push(row_rect((vx + vw).saturating_sub(2), y, 2), Target::Increase, body);
            }
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), body);

    let rule_style = panel_ink(theme, bg, 0.2);
    let rule_lines: Vec<Line> = if columns {
        (0..rule.height).map(|_| Line::from(Span::styled("│", rule_style))).collect()
    } else {
        vec![Line::from(Span::styled("─".repeat(rule.width as usize), rule_style))]
    };
    f.render_widget(Paragraph::new(rule_lines), rule);
    let text_w = text.width as usize;
    let mut text_lines: Vec<Line> = Vec::new();
    let desc_lines = wrap(&desc_text, text_w);
    let intro_lines = wrap(&intro_text, text_w);
    if columns && intro_lines.len() + 1 + desc_lines.len() <= text.height as usize {
        for l in intro_lines {
            text_lines.push(Line::from(Span::styled(l, panel_ink(theme, bg, 0.75))));
        }
        text_lines.push(Line::default());
    }
    for l in desc_lines {
        text_lines.push(Line::from(Span::styled(l, panel_ink(theme, bg, 0.6))));
    }
    f.render_widget(Paragraph::new(text_lines), text);
    let hint = if guide.editing.is_some() && folder {
        "↵ save · Tab complete · Esc cancel"
    } else if guide.editing.is_some() {
        "↵ save · Esc cancel"
    } else if guide.step == Step::Providers {
        "↑↓ move · Space on/off · K/J reorder · Esc back"
    } else if guide.step == Step::Player {
        "↑↓ move · ↵ select · Esc settings"
    } else {
        "↑↓ move · ←→ change · ↵ select · Esc back"
    };
    f.render_widget(
        Paragraph::new(truncate(hint, inner.width as usize)).style(panel_ink(theme, bg, 0.4)),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_on_words() {
        assert_eq!(wrap("one two three four", 9), vec!["one two", "three", "four"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert!(wrap("", 10).is_empty());
    }

    #[test]
    fn splits_left_when_wide_and_top_when_narrow() {
        let wide = split(Rect::new(0, 0, 160, 40));
        assert!(wide.side);
        assert_eq!(wide.panel.x, 0);
        assert_eq!(wide.main.x, wide.panel.width);
        assert_eq!(wide.panel.width + wide.main.width, 160);
        assert!(wide.main.width >= 96);

        let narrow = split(Rect::new(0, 0, 80, 40));
        assert!(!narrow.side);
        assert_eq!(narrow.panel.y, 0);
        assert_eq!(narrow.main.y, narrow.panel.height);
        assert_eq!(narrow.panel.height + narrow.main.height, 40);
        assert!(narrow.main.height >= 20);
    }

    #[test]
    fn steps_go_in_order() {
        assert_eq!(Step::Player.prev(), None);
        assert_eq!(Step::Player.next(), Some(Step::Providers));
        assert_eq!(Step::Done.next(), None);
    }
}
