use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{
    Alignment, Background, ClickSeek, Config, MatchTier, PlayerMode, Preset, Renderer, SPICY_KEY_ENV, ScrollAnimation,
    SyllableSpacing, VisualStyle,
};
use crate::providers::cache::MAX_TTL_DAYS;
use crate::providers::{ordered_ids, provider_name};

pub const PRESET_DESC: &str = "Changes the look in one go. Spicy Lyrics matches the Spicy Lyrics extension: real fonts with depth blur in the pixel renderer, an album-coloured gradient, syllables that grow on their own and a page that glides as one. Lyrminal is the default look.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsAction {
    PickPlayer,
    ClearCache,
    ResetDefaults,
    Onboarding,
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
        folder: bool,
    },
    Provider(String),
    Preset,
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

fn tolerance_desc() -> &'static str {
    "How far a search result's length may differ from the playing track and still count as the same recording."
}

pub const DEFAULT_LIBRARY_LABEL: &str = "Default sync library location";

pub fn default_library_item() -> Item {
    item(
        DEFAULT_LIBRARY_LABEL,
        "The folder where you keep your sync files. Importing from a file starts there, so you only type the file name. Enter to edit, Tab completes folders, Ctrl-U clears.",
        Kind::Text {
            get: |c| c.providers.local_library.default_dir.clone(),
            set: |c, v| {
                let v = v.trim();
                c.providers.local_library.default_dir = if v == "/" { v.to_string() } else { v.trim_end_matches('/').to_string() };
            },
            secret: false,
            placeholder: "~/Music/syncs",
            folder: true,
        },
    )
}

pub fn provider_page(id: &str) -> Vec<Item> {
    match id {
        crate::providers::local::ID => vec![
            item(
                "Enabled",
                "Use lyrics you added yourself. Press L to open the library and import a file, a URL or the clipboard for the playing track.",
                Kind::Toggle {
                    get: |c| c.providers.is_enabled(crate::providers::local::ID),
                    set: |c, v| c.providers.set_enabled(crate::providers::local::ID, v),
                },
            ),
            item(
                "Prioritize local library",
                "When a saved sync clearly matches the playing song, use it over the providers below the local library in the priority list, even if they have a better sync. Providers above it still win when they have a better sync.",
                Kind::Toggle {
                    get: |c| c.providers.local_library.prioritize,
                    set: |c, v| c.providers.local_library.prioritize = v,
                },
            ),
            default_library_item(),
            item(
                "Duration tolerance",
                "Lyrics saved for another copy of a song, such as the same track in a different player, are used when the title and artist match and the length differs by at most this much.",
                Kind::Number {
                    min: 0.5,
                    max: 10.0,
                    step: 0.5,
                    decimals: 1,
                    unit: "s",
                    get: |c| c.providers.local_library.duration_tolerance_secs,
                    set: |c, v| c.providers.local_library.duration_tolerance_secs = v,
                },
            ),
            item(
                "Formats",
                "LRC and enhanced LRC, TTML (Apple Music and AMLL), SRT, WebVTT, ASS/SSA, SBV, SAMI, JSON (Spotify, LRCLIB and Apple Music) and plain text. The format is detected automatically.",
                Kind::Info,
            ),
            item(
                "Storage",
                &format!(
                    "Saved lyrics and the library index live in {}. Lyrics that check for changes re-read their file or URL every time they load and fall back to the saved copy.",
                    crate::library::Library::default().dir().display()
                ),
                Kind::Info,
            ),
        ],
        crate::providers::spicy::ID => vec![
            item(
                "Enabled",
                "Use Spicy Lyrics. It returns community syncs first, then Apple Music and Spotify syncs.",
                Kind::Toggle {
                    get: |c| c.providers.is_enabled(crate::providers::spicy::ID),
                    set: |c, v| c.providers.set_enabled(crate::providers::spicy::ID, v),
                },
            ),
            item(
                "API key",
                &format!(
                    "Secret key from developers.spicylyrics.org → Dashboard → Applications. Stored in ~/.config/lyrminal/config.json. The {SPICY_KEY_ENV} environment variable overrides it."
                ),
                Kind::Text {
                    get: |c| c.providers.spicy_lyrics.api_key.clone(),
                    set: |c, v| c.providers.spicy_lyrics.api_key = v.trim().to_string(),
                    secret: true,
                    placeholder: "sl_sk_…",
                    folder: false,
                },
            ),
            item(
                "Auto-match",
                "When the player gives no Spotify id, the worst search match Spicy Lyrics may pick on its own. A different version of a song has different timings, so exact is safest.",
                Kind::Choice {
                    options: &["exact", "likely", "version", "other artist", "related"],
                    get: |c| MatchTier::ALL.iter().position(|t| *t == c.providers.spicy_lyrics.min_tier).unwrap_or(0),
                    set: |c, i| c.providers.spicy_lyrics.min_tier = MatchTier::ALL[i.min(4)],
                },
            ),
            item(
                "Lookups",
                "By Spotify id when the player exposes one (Spotify, spotifast), otherwise by searching title, artist, album and length.",
                Kind::Info,
            ),
            item(
                "Credits",
                "The provider and source are shown in the track header, or under the lyrics when the header is hidden. Community syncs also credit and link the people who made them, on one line under the lyrics.",
                Kind::Info,
            ),
        ],
        crate::providers::lrcred::ID => vec![
            item(
                "Enabled",
                "Use lrc.red: tens of millions of tracks, many word-synced, with duets and background vocals. No key needed.",
                Kind::Toggle {
                    get: |c| c.providers.is_enabled(crate::providers::lrcred::ID),
                    set: |c, v| c.providers.set_enabled(crate::providers::lrcred::ID, v),
                },
            ),
            item(
                "Duration tolerance",
                tolerance_desc(),
                Kind::Number {
                    min: 0.5,
                    max: 10.0,
                    step: 0.5,
                    decimals: 1,
                    unit: "s",
                    get: |c| c.providers.lrc_red.duration_tolerance_secs,
                    set: |c, v| c.providers.lrc_red.duration_tolerance_secs = v,
                },
            ),
            item(
                "Lookups",
                "By ISRC when the player exposes one, otherwise by searching title and artist. Only exact matches are picked automatically.",
                Kind::Info,
            ),
            item("Credits", "lrc.red is credited and linked under the lyrics, as its terms require.", Kind::Info),
        ],
        crate::providers::unison::ID => vec![
            item(
                "Enabled",
                "Use Better Lyrics' Unison: a free, public database of community-synced lyrics, many word-synced. No key needed.",
                Kind::Toggle {
                    get: |c| c.providers.is_enabled(crate::providers::unison::ID),
                    set: |c, v| c.providers.set_enabled(crate::providers::unison::ID, v),
                },
            ),
            item(
                "Duration tolerance",
                tolerance_desc(),
                Kind::Number {
                    min: 0.5,
                    max: 10.0,
                    step: 0.5,
                    decimals: 1,
                    unit: "s",
                    get: |c| c.providers.unison.duration_tolerance_secs,
                    set: |c, v| c.providers.unison.duration_tolerance_secs = v,
                },
            ),
            item(
                "Lookups",
                "By searching title and artist. Only exact matches are picked automatically, preferring the best sync.",
                Kind::Info,
            ),
            item(
                "Credits",
                "\"Lyrics from Unison · submitted by …\" is shown under the lyrics, linking to the song page and the submitter's profile, as Unison requires. Follow the song link to vote on, report or fix a sync.",
                Kind::Info,
            ),
        ],
        crate::providers::lrcmux::ID => vec![
            item(
                "Enabled",
                "Use lrcmux: a free lyrics API that gathers word- and line-synced lyrics from Deezer, KuGou and YouTube Music. No key needed.",
                Kind::Toggle {
                    get: |c| c.providers.is_enabled(crate::providers::lrcmux::ID),
                    set: |c, v| c.providers.set_enabled(crate::providers::lrcmux::ID, v),
                },
            ),
            item(
                "Duration tolerance",
                tolerance_desc(),
                Kind::Number {
                    min: 0.5,
                    max: 10.0,
                    step: 0.5,
                    decimals: 1,
                    unit: "s",
                    get: |c| c.providers.lrcmux.duration_tolerance_secs,
                    set: |c, v| c.providers.lrcmux.duration_tolerance_secs = v,
                },
            ),
            item(
                "Lookups",
                "By searching title, artist, album and length; lrcmux picks the best sync among its sources. Choosing a sync lists one result per source.",
                Kind::Info,
            ),
            item(
                "Credits",
                "The source lrcmux got the lyrics from, such as Deezer, is shown in the track header next to lrcmux.",
                Kind::Info,
            ),
        ],
        crate::providers::lrclib::ID => vec![
            item(
                "Enabled",
                "Use LRCLIB: a free, open lyrics database with line-synced and some word-synced lyrics. No key needed.",
                Kind::Toggle {
                    get: |c| c.providers.is_enabled(crate::providers::lrclib::ID),
                    set: |c, v| c.providers.set_enabled(crate::providers::lrclib::ID, v),
                },
            ),
            item(
                "Duration tolerance",
                tolerance_desc(),
                Kind::Number {
                    min: 0.5,
                    max: 10.0,
                    step: 0.5,
                    decimals: 1,
                    unit: "s",
                    get: |c| c.providers.lrclib.duration_tolerance_secs,
                    set: |c, v| c.providers.lrclib.duration_tolerance_secs = v,
                },
            ),
            item(
                "Lookups",
                "By title, artist, album and length, falling back to a search. Requests are spaced out to respect LRCLIB's limits.",
                Kind::Info,
            ),
        ],
        _ => Vec::new(),
    }
}

pub fn sections(cfg: &Config) -> Vec<Section> {
    let providers: Vec<Item> = ordered_ids(cfg)
        .into_iter()
        .map(|id| {
            let name = provider_name(&id).to_string();
            item(
                &name,
                "Enter opens this provider's settings. Space turns it on or off. Shift+↑/↓ or K/J changes its priority: lyrics come from the first provider that has them.",
                Kind::Provider(id),
            )
        })
        .collect();

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
                        folder: false,
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
                        folder: false,
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
                item("Preset", PRESET_DESC, Kind::Preset),
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
                    "Syllable spacing",
                    "Pixel renderer, Spring style. Joined keeps the syllables of a word touching as they grow and shrink. Separate scales each syllable on its own, like Spicy Lyrics, so small gaps open inside words.",
                    Kind::Choice {
                        options: &["joined", "separate"],
                        get: |c| (c.appearance.syllable_spacing == SyllableSpacing::Separate) as usize,
                        set: |c, i| {
                            c.appearance.syllable_spacing = if i == 1 { SyllableSpacing::Separate } else { SyllableSpacing::Joined }
                        },
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
                        folder: false,
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
            ],
        },
        Section {
            title: "Scrolling",
            items: vec![
                item(
                    "Animation",
                    "Staggered: each line follows the one above it with a short delay, rippling down the screen. Smooth: the whole page glides as one.",
                    Kind::Choice {
                        options: &["staggered", "smooth"],
                        get: |c| (c.scroll.animation == ScrollAnimation::Smooth) as usize,
                        set: |c, i| {
                            c.scroll.animation = if i == 1 { ScrollAnimation::Smooth } else { ScrollAnimation::Staggered }
                        },
                    },
                ),
                item(
                    "Speed",
                    "How fast the lyrics scroll to the next line. Higher is faster.",
                    Kind::Number {
                        min: 0.25,
                        max: 3.0,
                        step: 0.05,
                        decimals: 2,
                        unit: "×",
                        get: |c| c.scroll.speed,
                        set: |c, v| c.scroll.speed = v,
                    },
                ),
                item(
                    "Scroll ahead",
                    "Start moving to the next line this long before it is sung, so it is in place when it begins.",
                    Kind::Number {
                        min: 0.0,
                        max: 1500.0,
                        step: 50.0,
                        decimals: 0,
                        unit: "ms",
                        get: |c| c.scroll.lead_ms as f64,
                        set: |c, v| c.scroll.lead_ms = v as u32,
                    },
                ),
                item(
                    "Click to seek",
                    "Clicking the lyrics jumps the song there: to the start of the line, or to the clicked word when the sync has word timings. Needs playback controls and mouse input.",
                    Kind::Choice {
                        options: &["off", "lines", "words"],
                        get: |c| match c.scroll.click_seek {
                            ClickSeek::Off => 0,
                            ClickSeek::Lines => 1,
                            ClickSeek::Words => 2,
                        },
                        set: |c, i| {
                            c.scroll.click_seek = match i {
                                0 => ClickSeek::Off,
                                1 => ClickSeek::Lines,
                                _ => ClickSeek::Words,
                            }
                        },
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
                item(
                    "Mouse",
                    "Mouse wheel scrolling and clicking on the lyrics and progress bar. Turn off to keep the terminal's own text selection.",
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
                    "Prefer synced lyrics",
                    "Ask the providers for the best sync: word sync beats line sync, which beats unsynced lyrics. When several have the same kind, the priority order decides. Off: use the first provider that has any lyrics.",
                    Kind::Toggle {
                        get: |c| c.providers.prefer_synced,
                        set: |c, v| c.providers.prefer_synced = v,
                    },
                ),
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
                    "Fix background lines",
                    "On word-synced lyrics without background vocals, move words in parentheses into a background line.",
                    Kind::Toggle {
                        get: |c| c.lyrics.fix_background_lines,
                        set: |c, v| c.lyrics.fix_background_lines = v,
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
                item(
                    "Remember settings position",
                    "Reopening settings soon after closing them lands on the same tab and setting you left. Off always starts at the top.",
                    Kind::Toggle {
                        get: |c| c.controls.remember_settings,
                        set: |c, v| c.controls.remember_settings = v,
                    },
                ),
                item(
                    "Remember position for",
                    "How long after closing settings they reopen where you left them.",
                    Kind::Number {
                        min: 5.0,
                        max: 600.0,
                        step: 5.0,
                        decimals: 0,
                        unit: "s",
                        get: |c| c.controls.remember_settings_secs as f64,
                        set: |c, v| c.controls.remember_settings_secs = v as u32,
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
                    "Lyrminal is an unofficial app. It is not made, endorsed or supported by Spicy Lyrics, lrc.red, Better Lyrics (Unison), lrcmux or LRCLIB; it uses their public APIs.",
                    Kind::Info,
                ),
                item(
                    "Credits",
                    "Lyrics are credited to the provider that served them, in the track header or at the end of the lyrics, and community syncs to the people who made them.",
                    Kind::Info,
                ),
                item("Config file", &crate::config::Config::path().display().to_string(), Kind::Info),
                item(
                    "Welcome tour",
                    "Walk through setting up the player and providers again, then try the main looks on a song that is playing.",
                    Kind::Action(SettingsAction::Onboarding),
                ),
                item(
                    "Reset all settings",
                    "Restore every setting to its default, including API keys.",
                    Kind::Action(SettingsAction::ResetDefaults),
                ),
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
        Kind::Preset => Preset::detect(cfg).map(|p| p.label()).unwrap_or("custom").into(),
        Kind::Action(_) => "↵".into(),
        Kind::Info => String::new(),
    }
}

pub fn find_item(cfg: &Config, section: &str, label: &str) -> Option<Item> {
    sections(cfg)
        .into_iter()
        .find(|s| s.title == section)?
        .items
        .into_iter()
        .find(|i| i.label == label)
}

pub fn adjustable(kind: &Kind) -> bool {
    matches!(kind, Kind::Toggle { .. } | Kind::Choice { .. } | Kind::Number { .. } | Kind::Preset)
}

pub fn step_value(cfg: &mut Config, kind: &Kind, dir: i32, shift: bool) -> bool {
    match kind {
        Kind::Choice { options, get, set } => {
            let n = options.len() as i32;
            set(cfg, ((get(cfg) as i32 + dir).rem_euclid(n)) as usize);
            true
        }
        Kind::Number {
            min, max, step, get, set, ..
        } => {
            let mult = if shift { 10.0 } else { 1.0 };
            let v = (get(cfg) + dir as f64 * step * mult).clamp(*min, *max);
            set(cfg, (v / step).round() * step);
            true
        }
        Kind::Toggle { get, set } => {
            set(cfg, !get(cfg));
            true
        }
        Kind::Preset => {
            let n = Preset::ALL.len() as i32;
            let next = match Preset::detect(cfg).and_then(|p| Preset::ALL.iter().position(|q| *q == p)) {
                Some(i) => (i as i32 + dir).rem_euclid(n),
                None if dir > 0 => 0,
                None => n - 1,
            };
            Preset::ALL[next as usize].apply(cfg);
            true
        }
        _ => false,
    }
}

pub struct SettingsState {
    pub section: usize,
    pub item: usize,
    pub page: Option<String>,
    pub editing: Option<String>,
    pub confirm: Option<SettingsAction>,
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            section: 0,
            item: 0,
            page: None,
            editing: None,
            confirm: None,
        }
    }

    pub fn reopen(left: Option<(SettingsState, Instant)>, cfg: &Config, now: Instant) -> Self {
        let window = Duration::from_secs(cfg.controls.remember_settings_secs as u64);
        match left {
            Some((state, at)) if cfg.controls.remember_settings && now.saturating_duration_since(at) <= window => Self {
                editing: None,
                confirm: None,
                ..state
            },
            _ => Self::new(),
        }
    }

    pub fn items(&self, cfg: &Config) -> Vec<Item> {
        match &self.page {
            Some(id) => provider_page(id),
            None => {
                let mut secs = sections(cfg);
                let i = self.section.min(secs.len() - 1);
                secs.swap_remove(i).items
            }
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

fn reset_item(state: &mut SettingsState, cfg: &mut Config, kind: &Kind) -> bool {
    let d = Config::default();
    match kind {
        Kind::Toggle { get, set } => {
            let v = get(&d);
            let changed = get(cfg) != v;
            set(cfg, v);
            changed
        }
        Kind::Choice { get, set, .. } => {
            let v = get(&d);
            let changed = get(cfg) != v;
            set(cfg, v);
            changed
        }
        Kind::Number { get, set, .. } => {
            let v = get(&d);
            let changed = get(cfg) != v;
            set(cfg, v);
            changed
        }
        Kind::Text { get, set, .. } => {
            let v = get(&d);
            let changed = get(cfg) != v;
            set(cfg, v);
            changed
        }
        Kind::Provider(id) => {
            let before = (ordered_ids(cfg), cfg.providers.is_enabled(id));
            cfg.providers.set_enabled(id, d.providers.is_enabled(id));
            let want = ordered_ids(&d).iter().position(|p| p == id);
            let have = ordered_ids(cfg).iter().position(|p| p == id);
            if let (Some(want), Some(have)) = (want, have) {
                move_provider(cfg, id, want as i32 - have as i32);
                state.item = ordered_ids(cfg).iter().position(|p| p == id).unwrap_or(state.item);
            }
            before != (ordered_ids(cfg), cfg.providers.is_enabled(id))
        }
        Kind::Preset | Kind::Action(_) | Kind::Info => false,
    }
}

pub fn handle_key(state: &mut SettingsState, cfg: &mut Config, key: KeyEvent) -> SettingsOutcome {
    let secs = sections(cfg);
    state.section = state.section.min(secs.len() - 1);
    let items = state.items(cfg);
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
            KeyCode::Tab if matches!(current.kind, Kind::Text { folder: true, .. }) => {
                if let Some(done) = crate::ui::library::complete_path(buf, true) {
                    *buf = done;
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
    let leave_page = |state: &mut SettingsState, cfg: &Config| {
        if let Some(id) = state.page.take() {
            state.item = ordered_ids(cfg).iter().position(|p| *p == id).unwrap_or(0);
        }
    };
    match key.code {
        KeyCode::Esc if state.page.is_some() => leave_page(state, cfg),
        KeyCode::Esc | KeyCode::Char('q') => return SettingsOutcome::Close,
        KeyCode::Tab => {
            state.page = None;
            state.section = (state.section + 1) % secs.len();
            state.item = 0;
        }
        KeyCode::BackTab => {
            state.page = None;
            state.section = (state.section + secs.len() - 1) % secs.len();
            state.item = 0;
        }
        KeyCode::Right | KeyCode::Char('l') if matches!(current.kind, Kind::Provider(_)) => {
            if let Kind::Provider(id) = &current.kind {
                state.page = Some(id.clone());
                state.item = 0;
            }
        }
        KeyCode::Left | KeyCode::Char('h')
            if state.page.is_some()
                && !matches!(current.kind, Kind::Choice { .. } | Kind::Number { .. } | Kind::Toggle { .. }) =>
        {
            leave_page(state, cfg)
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
            if step_value(cfg, &current.kind, dir, shift) {
                return SettingsOutcome::Changed;
            }
            if dir < 0 {
                state.section = (state.section + secs.len() - 1) % secs.len();
            } else {
                state.section = (state.section + 1) % secs.len();
            }
            state.item = 0;
        }
        KeyCode::Enter | KeyCode::Char(' ') => match &current.kind {
            Kind::Toggle { .. } | Kind::Choice { .. } | Kind::Preset => {
                step_value(cfg, &current.kind, 1, false);
                return SettingsOutcome::Changed;
            }
            Kind::Text { get, secret, .. } => {
                state.editing = Some(if *secret { String::new() } else { get(cfg) });
            }
            Kind::Provider(id) => {
                if key.code == KeyCode::Enter {
                    state.page = Some(id.clone());
                    state.item = 0;
                } else {
                    let on = cfg.providers.is_enabled(id);
                    cfg.providers.set_enabled(id, !on);
                    return SettingsOutcome::Changed;
                }
            }
            Kind::Action(a) => match a {
                SettingsAction::PickPlayer | SettingsAction::Onboarding => return SettingsOutcome::Action(*a),
                _ => state.confirm = Some(*a),
            },
            Kind::Number { .. } | Kind::Info => {}
        },
        KeyCode::Char('r') => {
            if reset_item(state, cfg, &current.kind) {
                return SettingsOutcome::Changed;
            }
        }
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
        st.item = 5;
        handle_key(&mut st, &mut cfg, key(KeyCode::Char('K'), KeyModifiers::SHIFT));
        assert_eq!(
            ordered_ids(&cfg),
            vec!["local-library", "spicy-lyrics", "lrc-red", "unison", "lrclib", "lrcmux"]
        );
        assert_eq!(st.item, 4);
        handle_key(&mut st, &mut cfg, key(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(!cfg.providers.is_enabled("lrclib"));
    }

    #[test]
    fn providers_tab_lists_only_providers() {
        let cfg = Config::default();
        let items = &sections(&cfg)[0].items;
        assert_eq!(items.len(), 6);
        assert!(items.iter().all(|i| matches!(i.kind, Kind::Provider(_))));
        assert!(matches!(&items[0].kind, Kind::Provider(id) if id == crate::providers::local::ID));
    }

    #[test]
    fn adds_new_providers_at_their_default_position() {
        let mut cfg = Config::default();
        cfg.providers.order = vec!["lrclib".into(), "spicy-lyrics".into(), "lrc-red".into()];
        assert_eq!(
            ordered_ids(&cfg),
            vec!["local-library", "lrclib", "spicy-lyrics", "unison", "lrcmux", "lrc-red"]
        );
        cfg.providers.order = vec!["local-library".into(), "spicy-lyrics".into(), "lrc-red".into(), "lrclib".into()];
        assert_eq!(
            ordered_ids(&cfg),
            vec!["local-library", "spicy-lyrics", "lrc-red", "unison", "lrcmux", "lrclib"]
        );
        cfg.providers.order = vec![
            "local-library".into(),
            "spicy-lyrics".into(),
            "lrc-red".into(),
            "unison".into(),
            "lrclib".into(),
        ];
        assert_eq!(
            ordered_ids(&cfg),
            vec!["local-library", "spicy-lyrics", "lrc-red", "unison", "lrcmux", "lrclib"]
        );
    }

    #[test]
    fn sets_the_default_library_folder_with_completion() {
        let dir = std::env::temp_dir().join(format!("lyrminal-syncdir-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("my-syncs")).unwrap();
        std::fs::write(dir.join("my-song.lrc"), "").unwrap();
        let mut cfg = Config::default();
        let mut st = SettingsState::new();
        handle_key(&mut st, &mut cfg, key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(st.page.as_deref(), Some(crate::providers::local::ID));
        st.item = st.items(&cfg).iter().position(|i| i.label == DEFAULT_LIBRARY_LABEL).unwrap();
        handle_key(&mut st, &mut cfg, key(KeyCode::Enter, KeyModifiers::NONE));
        paste(&mut st, &format!("{}/my", dir.display()));
        handle_key(&mut st, &mut cfg, key(KeyCode::Tab, KeyModifiers::NONE));
        let out = handle_key(&mut st, &mut cfg, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(out, SettingsOutcome::Changed));
        let want = format!("{}/my-syncs", dir.display());
        assert_eq!(cfg.providers.local_library.default_dir, want);
        assert_eq!(cfg.providers.local_library.import_start(), Some(format!("{want}/")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edits_secret_text_in_provider_page() {
        let mut cfg = Config::default();
        let mut st = SettingsState::new();
        st.item = 1;
        handle_key(&mut st, &mut cfg, key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(st.page.as_deref(), Some("spicy-lyrics"));
        st.item = 1;
        handle_key(&mut st, &mut cfg, key(KeyCode::Enter, KeyModifiers::NONE));
        paste(&mut st, "sl_sk_abcd1234\n");
        let out = handle_key(&mut st, &mut cfg, key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(out, SettingsOutcome::Changed));
        assert_eq!(cfg.providers.spicy_lyrics.api_key, "sl_sk_abcd1234");
        let shown = value_text(&cfg, &provider_page("spicy-lyrics")[1].kind);
        assert!(shown.ends_with("1234") && !shown.contains("abcd"));
        handle_key(&mut st, &mut cfg, key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(st.page.is_none());
        assert_eq!(st.item, 1);
    }

    #[test]
    fn provider_pages_have_their_own_tolerance() {
        let mut cfg = Config::default();
        let mut st = SettingsState::new();
        st.item = 2;
        handle_key(&mut st, &mut cfg, key(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(st.page.as_deref(), Some("lrc-red"));
        st.item = 1;
        handle_key(&mut st, &mut cfg, key(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(cfg.providers.lrc_red.duration_tolerance_secs, 2.5);
        assert_eq!(cfg.providers.lrclib.duration_tolerance_secs, 2.0);
    }

    #[test]
    fn resets_selected_setting() {
        let mut cfg = Config::default();
        let mut st = SettingsState::new();
        st.section = sections(&cfg).iter().position(|s| s.title == "Sync").unwrap();
        cfg.sync.offset_ms = 420;
        cfg.sync.smoothing = !Config::default().sync.smoothing;
        let out = handle_key(&mut st, &mut cfg, key(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(matches!(out, SettingsOutcome::Changed));
        assert_eq!(cfg.sync.offset_ms, Config::default().sync.offset_ms);
        assert_ne!(cfg.sync.smoothing, Config::default().sync.smoothing);
        let out = handle_key(&mut st, &mut cfg, key(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(matches!(out, SettingsOutcome::None));
    }

    #[test]
    fn resets_provider_priority_and_state() {
        let mut cfg = Config::default();
        let mut st = SettingsState::new();
        st.item = 3;
        handle_key(&mut st, &mut cfg, key(KeyCode::Char('K'), KeyModifiers::SHIFT));
        handle_key(&mut st, &mut cfg, key(KeyCode::Char(' '), KeyModifiers::NONE));
        let out = handle_key(&mut st, &mut cfg, key(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(matches!(out, SettingsOutcome::Changed));
        assert_eq!(ordered_ids(&cfg), ordered_ids(&Config::default()));
        assert!(cfg.providers.is_enabled("lrclib"));
        assert_eq!(st.item, 3);
    }

    #[test]
    fn applies_presets_from_appearance() {
        let mut cfg = Config::default();
        let mut st = SettingsState::new();
        st.section = sections(&cfg).iter().position(|s| s.title == "Appearance").unwrap();
        assert_eq!(value_text(&cfg, &st.items(&cfg)[0].kind), "Lyrminal");
        let out = handle_key(&mut st, &mut cfg, key(KeyCode::Left, KeyModifiers::NONE));
        assert!(matches!(out, SettingsOutcome::Changed));
        assert_eq!(Preset::detect(&cfg), Some(Preset::SpicyLyrics));
        cfg.appearance.glow = true;
        assert_eq!(value_text(&cfg, &st.items(&cfg)[0].kind), "custom");
        handle_key(&mut st, &mut cfg, key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(Preset::detect(&cfg), Some(Preset::SpicyLyrics));
    }

    #[test]
    fn reopens_where_settings_were_left() {
        let mut cfg = Config::default();
        let at = Instant::now();
        let back = |cfg: &Config, after: u64| {
            let left = SettingsState {
                section: 0,
                item: 2,
                page: Some(crate::providers::lrcred::ID.into()),
                editing: Some("draft".into()),
                confirm: Some(SettingsAction::ClearCache),
            };
            SettingsState::reopen(Some((left, at)), cfg, at + Duration::from_secs(after))
        };
        let st = back(&cfg, 10);
        assert_eq!((st.section, st.item, st.page.as_deref()), (0, 2, Some(crate::providers::lrcred::ID)));
        assert!(st.editing.is_none() && st.confirm.is_none());
        assert_eq!(back(&cfg, 30).item, 2);
        assert_eq!(back(&cfg, 31).item, 0);
        cfg.controls.remember_settings_secs = 120;
        assert_eq!(back(&cfg, 90).item, 2);
        cfg.controls.remember_settings = false;
        let st = back(&cfg, 1);
        assert_eq!((st.section, st.item, st.page), (0, 0, None));
        assert_eq!(SettingsState::reopen(None, &Config::default(), at).item, 0);
    }

    #[test]
    fn clamps_cache_ttl() {
        let mut cfg = Config::default();
        cfg.cache.ttl_days = 30;
        let mut st = SettingsState::new();
        st.section = sections(&cfg).iter().position(|s| s.title == "Cache").unwrap();
        st.item = 1;
        handle_key(&mut st, &mut cfg, key(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(cfg.cache.ttl_days, 30);
    }
}
