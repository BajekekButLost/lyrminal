use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{
    Alignment, Background, Config, MatchTier, PlayerMode, Renderer, SPICY_KEY_ENV, VisualStyle,
};
use crate::providers::cache::MAX_TTL_DAYS;
use crate::providers::{KNOWN_PROVIDERS, ordered_ids, provider_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsAction {
    PickPlayer,
    ClearCache,
    ResetDefaults,
}

pub enum Kind {
    Toggle {
        get: fn(&Config) -> bool,
        set: fn(&mut Config, bool),
    },
    Choice {
        options: &'static [&'static str],
        get: fn(&Config) -> usize,
        set: fn(&mut Config, usize),
    },
    Number {
        min: f64,
        max: f64,
        step: f64,
        decimals: usize,
        unit: &'static str,
        get: fn(&Config) -> f64,
        set: fn(&mut Config, f64),
    },
    Text {
        get: fn(&Config) -> String,
        set: fn(&mut Config, String),
        secret: bool,
        placeholder: &'static str,
    },
    Provider(String),
    Action(SettingsAction),
    Info,
}

pub struct Item {
    pub label: String,
    pub desc: String,
    pub kind: Kind,
}

pub struct Section {
    pub title: &'static str,
    pub items: Vec<Item>,
}

fn item(label: &str, desc: &str, kind: Kind) -> Item {
    Item {
        label: label.into(),
        desc: desc.into(),
        kind,
    }
}

pub fn sections(cfg: &Config) -> Vec<Section> {
    let mut providers: Vec<Item> = ordered_ids(cfg)
        .into_iter()
        .map(|id| {
            let needs_key = KNOWN_PROVIDERS.iter().any(|p| p.id == id && p.needs_key);
            let name = provider_name(&id).to_string();
            item(
                &name,
                &format!(
                    "Space toggles this provider. Shift+↑/↓ or K/J changes its priority; lyrics come from the first provider that has them.{}",
                    if needs_key { " Needs an API key (below)." } else { "" }
                ),
                Kind::Provider(id),
            )
        })
        .collect();
    providers.extend([
        item(
            "Spicy Lyrics API key",
            &format!(
                "Secret key from developers.spicylyrics.org → Dashboard → Applications. Stored in ~/.config/lyrminal/config.json. The {SPICY_KEY_ENV} environment variable overrides it."
            ),
            Kind::Text {
                get: |c| c.providers.spicy_lyrics.api_key.clone(),
                set: |c, v| c.providers.spicy_lyrics.api_key = v.trim().to_string(),
                secret: true,
                placeholder: "sl_sk_…",
            },
        ),
        item(
            "Spicy Lyrics auto-match",
            "When the player gives no Spotify id, the worst search match Spicy Lyrics may pick on its own. A different version of a song has different timings, so exact is safest.",
            Kind::Choice {
                options: &["exact", "likely", "version", "other artist", "related"],
                get: |c| MatchTier::ALL.iter().position(|t| *t == c.providers.spicy_lyrics.min_tier).unwrap_or(0),
                set: |c, i| c.providers.spicy_lyrics.min_tier = MatchTier::ALL[i.min(4)],
            },
        ),
        item(
            "Duration tolerance",
            "How far a search result's length may differ from the playing track and still count as the same recording.",
            Kind::Number {
                min: 0.5,
                max: 10.0,
                step: 0.5,
                decimals: 1,
                unit: "s",
                get: |c| c.providers.duration_tolerance_secs,
                set: |c, v| c.providers.duration_tolerance_secs = v,
            },
        ),
        item(
            "Prefer synced lyrics",
            "If a higher-priority provider only has unsynced lyrics, keep asking the next ones for a synced version.",
            Kind::Toggle {
                get: |c| c.providers.prefer_synced,
                set: |c, v| c.providers.prefer_synced = v,
            },
        ),
    ]);

    vec![
        Section {
            title: "Providers",
            items: providers,
        },
        Section {
            title: "Player",
            items: vec![
                item(
                    "Follow",
                    "Auto follows whichever player is playing, preferring the list below. Fixed only follows the process named below.",
                    Kind::Choice {
                        options: &["auto", "fixed process"],
                        get: |c| (c.player.mode == PlayerMode::Fixed) as usize,
                        set: |c, i| c.player.mode = if i == 1 { PlayerMode::Fixed } else { PlayerMode::Auto },
                    },
                ),
                item(
                    "Fixed player",
                    "Process name, MPRIS bus name or player identity to follow in fixed mode, e.g. spotifast.",
                    Kind::Text {
                        get: |c| c.player.target.clone(),
                        set: |c, v| c.player.target = v.trim().to_string(),
                        secret: false,
                        placeholder: "spotifast",
                    },
                ),
                item(
                    "Pick a running player…",
                    "Choose from the players currently on the media bus.",
                    Kind::Action(SettingsAction::PickPlayer),
                ),
                item(
                    "Preferred players",
                    "Comma-separated, in order. Used by auto mode when several players are playing.",
                    Kind::Text {
                        get: |c| c.player.preferred.join(", "),
                        set: |c, v| {
                            c.player.preferred = v
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect()
                        },
                        secret: false,
                        placeholder: "spotifast, spotify",
                    },
                ),
                item(
                    "Position poll interval",
                    "How often the playback position is re-read from the player. Between reads it is extrapolated.",
                    Kind::Number {
                        min: 100.0,
                        max: 5000.0,
                        step: 100.0,
                        decimals: 0,
                        unit: "ms",
                        get: |c| c.player.poll_interval_ms as f64,
                        set: |c, v| c.player.poll_interval_ms = v as u64,
                    },
                ),
            ],
        },
        Section {
            title: "Appearance",
            items: vec![
                item(
                    "Renderer",
                    "Cell draws with terminal text. Pixel draws real fonts with blur, scale and glow through the kitty graphics protocol (kitty, Ghostty, WezTerm).",
                    Kind::Choice {
                        options: &["cell", "pixel"],
                        get: |c| (c.appearance.renderer == Renderer::Pixel) as usize,
                        set: |c, i| c.appearance.renderer = if i == 1 { Renderer::Pixel } else { Renderer::Cell },
                    },
                ),
                item(
                    "Style",
                    "Spring: per-word gradient wipe with glow and springy emphasis. Bloom: one soft edge sweeping the line, rippling scroll and flame emphasis.",
                    Kind::Choice {
                        options: &["spring", "bloom"],
                        get: |c| (c.appearance.style == VisualStyle::Bloom) as usize,
                        set: |c, i| c.appearance.style = if i == 1 { VisualStyle::Bloom } else { VisualStyle::Spring },
                    },
                ),
                item(
                    "Album art theming",
                    "Colour the lyrics and background from the album cover.",
                    Kind::Toggle {
                        get: |c| c.appearance.album_theming,
                        set: |c, v| c.appearance.album_theming = v,
                    },
                ),
                item(
                    "Background",
                    "What to paint behind the lyrics when album art theming is on. None keeps the terminal background.",
                    Kind::Choice {
                        options: &["none", "solid", "gradient"],
                        get: |c| match c.appearance.background {
                            Background::None => 0,
                            Background::Solid => 1,
                            Background::Gradient => 2,
                        },
                        set: |c, i| {
                            c.appearance.background = match i {
                                0 => Background::None,
                                1 => Background::Solid,
                                _ => Background::Gradient,
                            }
                        },
                    },
                ),
                item(
                    "Cover art",
                    "Show the album cover in the header. Needs a terminal with kitty graphics; otherwise nothing is shown.",
                    Kind::Toggle {
                        get: |c| c.appearance.album_art,
                        set: |c, v| c.appearance.album_art = v,
                    },
                ),
                item(
                    "Track header",
                    "Title, artist and album above the lyrics.",
                    Kind::Toggle {
                        get: |c| c.appearance.show_header,
                        set: |c, v| c.appearance.show_header = v,
                    },
                ),
                item(
                    "Progress bar",
                    "Song position below the lyrics.",
                    Kind::Toggle {
                        get: |c| c.appearance.show_progress,
                        set: |c, v| c.appearance.show_progress = v,
                    },
                ),
                item(
                    "Status bar",
                    "Player, offset and key hints at the bottom.",
                    Kind::Toggle {
                        get: |c| c.appearance.show_status,
                        set: |c, v| c.appearance.show_status = v,
                    },
                ),
                item(
                    "Alignment",
                    "Left keeps duets on opposite sides. Center centres every line.",
                    Kind::Choice {
                        options: &["left", "center"],
                        get: |c| (c.appearance.alignment == Alignment::Center) as usize,
                        set: |c, i| c.appearance.alignment = if i == 1 { Alignment::Center } else { Alignment::Left },
                    },
                ),
                item(
                    "Line gap",
                    "Blank rows between lyric lines.",
                    Kind::Number {
                        min: 0.0,
                        max: 3.0,
                        step: 1.0,
                        decimals: 0,
                        unit: "",
                        get: |c| c.appearance.line_gap as f64,
                        set: |c, v| c.appearance.line_gap = v as u16,
                    },
                ),
                item(
                    "Depth dimming",
                    "Lines further from the current one get fainter, standing in for blur in the cell renderer.",
                    Kind::Toggle {
                        get: |c| c.appearance.depth_dimming,
                        set: |c, v| c.appearance.depth_dimming = v,
                    },
                ),
                item(
                    "Fade sung lines",
                    "Lines fade away once sung.",
                    Kind::Toggle {
                        get: |c| c.appearance.fade_previous,
                        set: |c, v| c.appearance.fade_previous = v,
                    },
                ),
                item(
                    "Glow",
                    "Words brighten toward the accent colour as they are sung.",
                    Kind::Toggle {
                        get: |c| c.appearance.glow,
                        set: |c, v| c.appearance.glow = v,
                    },
                ),
                item(
                    "Emphasis on held words",
                    "Words held for a second or more animate letter by letter.",
                    Kind::Toggle {
                        get: |c| c.appearance.emphasis,
                        set: |c, v| c.appearance.emphasis = v,
                    },
                ),
                item(
                    "Contrast",
                    "Multiplies how bright unsung and sung lines are.",
                    Kind::Number {
                        min: 0.5,
                        max: 2.5,
                        step: 0.05,
                        decimals: 2,
                        unit: "×",
                        get: |c| c.appearance.contrast as f64,
                        set: |c, v| c.appearance.contrast = v as f32,
                    },
                ),
                item(
                    "Frame rate",
                    "Animation frame rate of the cell renderer.",
                    Kind::Number {
                        min: 15.0,
                        max: 144.0,
                        step: 5.0,
                        decimals: 0,
                        unit: "fps",
                        get: |c| c.appearance.fps as f64,
                        set: |c, v| c.appearance.fps = v as u32,
                    },
                ),
                item(
                    "Pixel frame rate",
                    "Frame rate of the pixel renderer. Each frame is a full image, so keep it modest.",
                    Kind::Number {
                        min: 10.0,
                        max: 60.0,
                        step: 5.0,
                        decimals: 0,
                        unit: "fps",
                        get: |c| c.appearance.pixel_fps as f64,
                        set: |c, v| c.appearance.pixel_fps = v as u32,
                    },
                ),
                item(
                    "Pixel font",
                    "Font family for the pixel renderer. Empty uses the system sans-serif font.",
                    Kind::Text {
                        get: |c| c.appearance.font_family.clone(),
                        set: |c, v| c.appearance.font_family = v.trim().to_string(),
                        secret: false,
                        placeholder: "sans-serif",
                    },
                ),
                item(
                    "Pixel font weight",
                    "100 is thin, 400 regular, 700 bold, 900 black.",
                    Kind::Number {
                        min: 100.0,
                        max: 900.0,
                        step: 100.0,
                        decimals: 0,
                        unit: "",
                        get: |c| c.appearance.font_weight as f64,
                        set: |c, v| c.appearance.font_weight = v as u16,
                    },
                ),
                item(
                    "Pixel font size",
                    "Lyrics size in the pixel renderer, in terminal rows.",
                    Kind::Number {
                        min: 1.0,
                        max: 6.0,
                        step: 0.1,
                        decimals: 1,
                        unit: "rows",
                        get: |c| c.appearance.font_scale as f64,
                        set: |c, v| c.appearance.font_scale = v as f32,
                    },
                ),
                item(
                    "Mouse wheel",
                    "Scroll the lyrics with the mouse wheel. Turn off to keep the terminal's own text selection.",
                    Kind::Toggle {
                        get: |c| c.appearance.mouse,
                        set: |c, v| c.appearance.mouse = v,
                    },
                ),
            ],
        },
        Section {
            title: "Lyrics",
            items: vec![
                item(
                    "Romanization",
                    "Show a romanized line under lyrics that have one.",
                    Kind::Toggle {
                        get: |c| c.lyrics.romanization,
                        set: |c, v| c.lyrics.romanization = v,
                    },
                ),
                item(
                    "Translation",
                    "Show a translated line under lyrics that have one.",
                    Kind::Toggle {
                        get: |c| c.lyrics.translation,
                        set: |c, v| c.lyrics.translation = v,
                    },
                ),
                item(
                    "Background vocals",
                    "Show background vocal lines.",
                    Kind::Toggle {
                        get: |c| c.lyrics.background_vocals,
                        set: |c, v| c.lyrics.background_vocals = v,
                    },
                ),
                item(
                    "Interlude dots",
                    "Show animated dots during instrumental breaks.",
                    Kind::Toggle {
                        get: |c| c.lyrics.interludes,
                        set: |c, v| c.lyrics.interludes = v,
                    },
                ),
                item(
                    "Interlude threshold",
                    "The shortest gap between lines that counts as an instrumental break.",
                    Kind::Number {
                        min: 1.0,
                        max: 15.0,
                        step: 0.5,
                        decimals: 1,
                        unit: "s",
                        get: |c| c.lyrics.interlude_threshold_secs,
                        set: |c, v| c.lyrics.interlude_threshold_secs = v,
                    },
                ),
                item(
                    "Join split words",
                    "Merge syllables of one word into a single unit for a smoother wipe.",
                    Kind::Toggle {
                        get: |c| c.lyrics.join_split_words,
                        set: |c, v| c.lyrics.join_split_words = v,
                    },
                ),
                item(
                    "Resume following after",
                    "Seconds after you stop scrolling before the lyrics follow the song again.",
                    Kind::Number {
                        min: 1.0,
                        max: 60.0,
                        step: 1.0,
                        decimals: 0,
                        unit: "s",
                        get: |c| c.lyrics.resume_follow_secs,
                        set: |c, v| c.lyrics.resume_follow_secs = v,
                    },
                ),
            ],
        },
        Section {
            title: "Sync",
            items: vec![
                item(
                    "Global offset",
                    "Shifts every song's lyrics. Positive shows lines earlier. Per-track offsets ([ and ]) add to this.",
                    Kind::Number {
                        min: -3000.0,
                        max: 3000.0,
                        step: 10.0,
                        decimals: 0,
                        unit: "ms",
                        get: |c| c.sync.offset_ms as f64,
                        set: |c, v| c.sync.offset_ms = v as i64,
                    },
                ),
                item(
                    "Smoothing",
                    "Glide small position corrections from the player instead of jumping.",
                    Kind::Toggle {
                        get: |c| c.sync.smoothing,
                        set: |c, v| c.sync.smoothing = v,
                    },
                ),
                item(
                    "Snap threshold",
                    "Corrections larger than this jump immediately, such as after a seek.",
                    Kind::Number {
                        min: 100.0,
                        max: 3000.0,
                        step: 50.0,
                        decimals: 0,
                        unit: "ms",
                        get: |c| c.sync.snap_threshold_ms as f64,
                        set: |c, v| c.sync.snap_threshold_ms = v as u64,
                    },
                ),
            ],
        },
        Section {
            title: "Controls",
            items: vec![
                item(
                    "Playback controls",
                    "Let Lyrminal play, pause, seek and skip through the system media interface. Off keeps it read-only.",
                    Kind::Toggle {
                        get: |c| c.controls.enabled,
                        set: |c, v| c.controls.enabled = v,
                    },
                ),
                item(
                    "Seek step",
                    "How far h / l and the arrow keys seek.",
                    Kind::Number {
                        min: 1.0,
                        max: 60.0,
                        step: 1.0,
                        decimals: 0,
                        unit: "s",
                        get: |c| c.controls.seek_step_secs as f64,
                        set: |c, v| c.controls.seek_step_secs = v as u32,
                    },
                ),
            ],
        },
        Section {
            title: "Cache",
            items: vec![
                item(
                    "Cache lyrics",
                    "Keep fetched lyrics on disk so repeat plays need no requests.",
                    Kind::Toggle {
                        get: |c| c.cache.enabled,
                        set: |c, v| c.cache.enabled = v,
                    },
                ),
                item(
                    "Keep cached lyrics for",
                    "Cached lyrics are refetched after this. Spicy Lyrics' terms cap storage at 30 days.",
                    Kind::Number {
                        min: 1.0,
                        max: MAX_TTL_DAYS as f64,
                        step: 1.0,
                        decimals: 0,
                        unit: "days",
                        get: |c| c.cache.ttl_days as f64,
                        set: |c, v| c.cache.ttl_days = v as u32,
                    },
                ),
                item("Clear cache", "Delete every cached lyric and lookup.", Kind::Action(SettingsAction::ClearCache)),
                item(
                    "Reset all settings",
                    "Restore every setting to its default. Your API key is kept.",
                    Kind::Action(SettingsAction::ResetDefaults),
                ),
            ],
        },
        Section {
            title: "About",
            items: vec![
                item(
                    &format!("Lyrminal v{}", env!("CARGO_PKG_VERSION")),
                    "A terminal lyrics viewer.",
                    Kind::Info,
                ),
                item(
                    "Unofficial",
                    "Lyrminal is an unofficial app. It is not made, endorsed or supported by Spicy Lyrics, lrc.red or LRCLIB; it uses their public APIs.",
                    Kind::Info,
                ),
                item(
                    "Credits",
                    "Lyrics are credited to the provider that served them, and community syncs to the people who made them, at the end of every song's lyrics.",
                    Kind::Info,
                ),
                item("Config file", &crate::config::Config::path().display().to_string(), Kind::Info),
            ],
        },
    ]
}

pub fn value_text(cfg: &Config, kind: &Kind) -> String {
    match kind {
        Kind::Toggle { get, .. } => if get(cfg) { "on" } else { "off" }.into(),
        Kind::Choice { options, get, .. } => options.get(get(cfg)).copied().unwrap_or("?").into(),
        Kind::Number {
            get, decimals, unit, ..
        } => {
            let v = get(cfg);
            let n = format!("{v:.decimals$}", decimals = *decimals);
            if unit.is_empty() { n } else { format!("{n} {unit}") }
        }
        Kind::Text {
            get,
            secret,
            placeholder,
            ..
        } => {
            let v = get(cfg);
            if *secret && cfg.providers.spicy_lyrics.key_from_env() {
                "set by environment".into()
            } else if v.is_empty() {
                format!("({placeholder})")
            } else if *secret {
                let tail: String = v.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                format!("••••••{tail}")
            } else {
                v
            }
        }
        Kind::Provider(id) => {
            let pos = ordered_ids(cfg).iter().position(|p| p == id).map(|p| p + 1).unwrap_or(0);
            let on = cfg.providers.is_enabled(id);
            let ready = if id == crate::providers::spicy::ID && cfg.providers.spicy_lyrics.effective_key().is_none() {
                " · needs key"
            } else {
                ""
            };
            format!("#{pos} {}{ready}", if on { "on" } else { "off" })
        }
        Kind::Action(_) => "↵".into(),
        Kind::Info => String::new(),
    }
}

pub struct SettingsState {
    pub section: usize,
    pub item: usize,
    pub editing: Option<String>,
    pub confirm: Option<SettingsAction>,
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            section: 0,
            item: 0,
            editing: None,
            confirm: None,
        }
    }
}

pub enum SettingsOutcome {
    None,
    Close,
    Changed,
    Action(SettingsAction),
}

fn move_provider(cfg: &mut Config, id: &str, delta: i32) {
    let mut ids = ordered_ids(cfg);
    let Some(pos) = ids.iter().position(|p| p == id) else { return };
    let new = (pos as i32 + delta).clamp(0, ids.len() as i32 - 1) as usize;
    if new != pos {
        let p = ids.remove(pos);
        ids.insert(new, p);
        cfg.providers.order = ids;
    }
}

pub fn handle_key(state: &mut SettingsState, cfg: &mut Config, key: KeyEvent) -> SettingsOutcome {
    let secs = sections(cfg);
    state.section = state.section.min(secs.len() - 1);
    let items = &secs[state.section].items;
    state.item = state.item.min(items.len().saturating_sub(1));
    let current = &items[state.item];

    if let Some(buf) = state.editing.as_mut() {
        match key.code {
            KeyCode::Esc => state.editing = None,
            KeyCode::Enter => {
                let value = buf.clone();
                state.editing = None;
                if let Kind::Text { set, .. } = &current.kind {
                    set(cfg, value);
                    return SettingsOutcome::Changed;
                }
            }
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => buf.clear(),
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let trimmed = buf.trim_end().len();
                buf.truncate(trimmed);
                while buf.chars().last().is_some_and(|c| !c.is_whitespace()) {
                    buf.pop();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => buf.push(c),
            _ => {}
        }
        return SettingsOutcome::None;
    }

    if let Some(action) = state.confirm {
        state.confirm = None;
        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter) {
            return SettingsOutcome::Action(action);
        }
        return SettingsOutcome::None;
    }

    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('o') => return SettingsOutcome::Close,
        KeyCode::Tab => {
            state.section = (state.section + 1) % secs.len();
            state.item = 0;
        }
        KeyCode::BackTab => {
            state.section = (state.section + secs.len() - 1) % secs.len();
            state.item = 0;
        }
        KeyCode::Up | KeyCode::Down | KeyCode::Char('K') | KeyCode::Char('J')
            if (shift || matches!(key.code, KeyCode::Char('K') | KeyCode::Char('J')))
                && matches!(current.kind, Kind::Provider(_)) =>
        {
            if let Kind::Provider(id) = &current.kind {
                let delta = if matches!(key.code, KeyCode::Up | KeyCode::Char('K')) { -1 } else { 1 };
                let id = id.clone();
                move_provider(cfg, &id, delta);
                let new_pos = ordered_ids(cfg).iter().position(|p| *p == id).unwrap_or(state.item);
                state.item = new_pos;
                return SettingsOutcome::Changed;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => state.item = (state.item + 1).min(items.len() - 1),
        KeyCode::Up | KeyCode::Char('k') => state.item = state.item.saturating_sub(1),
        KeyCode::Home | KeyCode::Char('g') => state.item = 0,
        KeyCode::End | KeyCode::Char('G') => state.item = items.len() - 1,
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
            let dir: i32 = if matches!(key.code, KeyCode::Left | KeyCode::Char('h')) { -1 } else { 1 };
            match &current.kind {
                Kind::Choice { options, get, set } => {
                    let n = options.len() as i32;
                    set(cfg, ((get(cfg) as i32 + dir).rem_euclid(n)) as usize);
                    return SettingsOutcome::Changed;
                }
                Kind::Number {
                    min, max, step, get, set, ..
                } => {
                    let mult = if shift { 10.0 } else { 1.0 };
                    let v = (get(cfg) + dir as f64 * step * mult).clamp(*min, *max);
                    set(cfg, (v / step).round() * step);
                    return SettingsOutcome::Changed;
                }
                Kind::Toggle { get, set } => {
                    set(cfg, !get(cfg));
                    return SettingsOutcome::Changed;
                }
                _ => {
                    if dir < 0 {
                        state.section = (state.section + secs.len() - 1) % secs.len();
                    } else {
                        state.section = (state.section + 1) % secs.len();
                    }
                    state.item = 0;
                }
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => match &current.kind {
            Kind::Toggle { get, set } => {
                set(cfg, !get(cfg));
                return SettingsOutcome::Changed;
            }
            Kind::Choice { options, get, set } => {
                set(cfg, (get(cfg) + 1) % options.len());
                return SettingsOutcome::Changed;
            }
            Kind::Text { get, secret, .. } => {
                state.editing = Some(if *secret { String::new() } else { get(cfg) });
            }
            Kind::Provider(id) => {
                let on = cfg.providers.is_enabled(id);
                cfg.providers.set_enabled(id, !on);
                return SettingsOutcome::Changed;
            }
            Kind::Action(a) => match a {
                SettingsAction::PickPlayer => return SettingsOutcome::Action(*a),
                _ => state.confirm = Some(*a),
            },
            Kind::Number { .. } | Kind::Info => {}
        },
        KeyCode::Delete | KeyCode::Backspace => {
            if let Kind::Text { set, .. } = &current.kind {
                set(cfg, String::new());
                return SettingsOutcome::Changed;
            }
        }
        _ => {}
    }
    SettingsOutcome::None
}

pub fn paste(state: &mut SettingsState, text: &str) {
    if let Some(buf) = state.editing.as_mut() {
        buf.push_str(text.trim_matches(['\n', '\r']));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn reorders_providers() {
        let mut cfg = Config::default();
        let mut st = SettingsState::new();
        st.item = 2;
        handle_key(&mut st, &mut cfg, key(KeyCode::Char('K'), KeyModifiers::SHIFT));
        assert_eq!(ordered_ids(&cfg), vec!["spicy-lyrics", "lrclib", "lrc-red"]);
        assert_eq!(st.item, 1);
        handle_key(&mut st, &mut cfg, key(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(!cfg.providers.is_enabled("lrclib"));
    }

    #[test]
    fn edits_secret_text() {
        let mut cfg = Config::default();
        let mut st = SettingsState::new();
        st.item = 3;
        handle_key(&mut st, &mut cfg, key(KeyCode::Enter, KeyModifiers::NONE));
        paste(&mut st, "sl_sk_abcd1234\n");
        let out = handle_key(&mut st, &mut cfg, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(out, SettingsOutcome::Changed));
        assert_eq!(cfg.providers.spicy_lyrics.api_key, "sl_sk_abcd1234");
        let shown = value_text(&cfg, &sections(&cfg)[0].items[3].kind);
        assert!(shown.ends_with("1234") && !shown.contains("abcd"));
    }

    #[test]
    fn clamps_cache_ttl() {
        let mut cfg = Config::default();
        cfg.cache.ttl_days = 30;
        let mut st = SettingsState::new();
        st.section = 6;
        st.item = 1;
        handle_key(&mut st, &mut cfg, key(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(cfg.cache.ttl_days, 30);
    }
}
