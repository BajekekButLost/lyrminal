use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const SPICY_KEY_ENV: &str = "SPICY_LYRICS_SECRET_KEY";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub player: PlayerConfig,
    pub providers: ProvidersConfig,
    pub appearance: AppearanceConfig,
    pub lyrics: LyricsConfig,
    pub scroll: ScrollConfig,
    pub sync: SyncConfig,
    pub controls: ControlsConfig,
    pub cache: CacheConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScrollAnimation {
    Staggered,
    Smooth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClickSeek {
    Off,
    Lines,
    Words,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScrollConfig {
    pub animation: ScrollAnimation,
    pub speed: f64,
    pub lead_ms: u32,
    pub click_seek: ClickSeek,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            animation: ScrollAnimation::Staggered,
            speed: 1.0,
            lead_ms: 300,
            click_seek: ClickSeek::Lines,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayerMode {
    Auto,
    Fixed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlayerConfig {
    pub mode: PlayerMode,
    pub target: String,
    pub preferred: Vec<String>,
    pub poll_interval_ms: u64,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            mode: PlayerMode::Auto,
            target: String::new(),
            preferred: vec!["spotifast".into(), "spotify".into()],
            poll_interval_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchTier {
    Exact,
    Likely,
    Version,
    OtherArtist,
    Related,
}

impl MatchTier {
    pub const ALL: [MatchTier; 5] = [
        MatchTier::Exact,
        MatchTier::Likely,
        MatchTier::Version,
        MatchTier::OtherArtist,
        MatchTier::Related,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MatchTier::Exact => "exact",
            MatchTier::Likely => "likely",
            MatchTier::Version => "version",
            MatchTier::OtherArtist => "other artist",
            MatchTier::Related => "related",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProvidersConfig {
    pub order: Vec<String>,
    pub disabled: Vec<String>,
    pub spicy_lyrics: SpicyConfig,
    pub lrc_red: SearchProviderConfig,
    pub lrclib: SearchProviderConfig,
    pub prefer_synced: bool,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            order: vec!["spicy-lyrics".into(), "lrc-red".into(), "lrclib".into()],
            disabled: Vec::new(),
            spicy_lyrics: SpicyConfig::default(),
            lrc_red: SearchProviderConfig::default(),
            lrclib: SearchProviderConfig::default(),
            prefer_synced: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchProviderConfig {
    pub duration_tolerance_secs: f64,
}

impl Default for SearchProviderConfig {
    fn default() -> Self {
        Self {
            duration_tolerance_secs: 2.0,
        }
    }
}

impl ProvidersConfig {
    pub fn is_enabled(&self, id: &str) -> bool {
        !self.disabled.iter().any(|d| d == id)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        self.disabled.retain(|d| d != id);
        if !enabled {
            self.disabled.push(id.to_string());
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SpicyConfig {
    pub api_key: String,
    pub min_tier: MatchTier,
}

impl Default for SpicyConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            min_tier: MatchTier::Exact,
        }
    }
}

impl SpicyConfig {
    pub fn effective_key(&self) -> Option<String> {
        if let Ok(k) = std::env::var(SPICY_KEY_ENV) {
            let k = k.trim().to_string();
            if !k.is_empty() {
                return Some(k);
            }
        }
        let k = self.api_key.trim();
        (!k.is_empty()).then(|| k.to_string())
    }

    pub fn key_from_env(&self) -> bool {
        std::env::var(SPICY_KEY_ENV)
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Renderer {
    Cell,
    Pixel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VisualStyle {
    Spring,
    Bloom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Background {
    None,
    Solid,
    Gradient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Alignment {
    Left,
    Center,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppearanceConfig {
    pub renderer: Renderer,
    pub style: VisualStyle,
    pub album_theming: bool,
    pub background: Background,
    pub album_art: bool,
    pub show_header: bool,
    pub show_progress: bool,
    pub show_status: bool,
    pub alignment: Alignment,
    pub line_gap: u16,
    pub depth_dimming: bool,
    pub fade_previous: bool,
    pub contrast: f32,
    pub fps: u32,
    pub pixel_fps: u32,
    pub font_family: String,
    pub font_weight: u16,
    pub font_scale: f32,
    pub glow: bool,
    pub emphasis: bool,
    pub mouse: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            renderer: Renderer::Cell,
            style: VisualStyle::Spring,
            album_theming: true,
            background: Background::Gradient,
            album_art: true,
            show_header: true,
            show_progress: true,
            show_status: true,
            alignment: Alignment::Left,
            line_gap: 1,
            depth_dimming: true,
            fade_previous: false,
            contrast: 1.0,
            fps: 60,
            pixel_fps: 30,
            font_family: String::new(),
            font_weight: 700,
            font_scale: 2.2,
            glow: false,
            emphasis: true,
            mouse: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LyricsConfig {
    pub romanization: bool,
    pub translation: bool,
    pub background_vocals: bool,
    pub interludes: bool,
    pub interlude_threshold_secs: f64,
    pub join_split_words: bool,
    pub resume_follow_secs: f64,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        Self {
            romanization: false,
            translation: false,
            background_vocals: true,
            interludes: true,
            interlude_threshold_secs: 3.0,
            join_split_words: false,
            resume_follow_secs: 4.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncConfig {
    pub offset_ms: i64,
    pub smoothing: bool,
    pub snap_threshold_ms: u64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            offset_ms: 0,
            smoothing: true,
            snap_threshold_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ControlsConfig {
    pub enabled: bool,
    pub seek_step_secs: u32,
}

impl Default for ControlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            seek_step_secs: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CacheConfig {
    pub enabled: bool,
    pub ttl_days: u32,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl_days: 7,
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lyrminal")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("lyrminal")
}

pub fn state_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lyrminal")
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.json")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("invalid config at {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text)?;
        restrict_permissions(&tmp);
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}
