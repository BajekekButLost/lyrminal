use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    pub updates: UpdatesConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdatesConfig {
    pub check: bool,
    pub auto_install: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            check: true,
            auto_install: false,
        }
    }
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
            lead_ms: 250,
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
    pub local_library: LocalLibraryConfig,
    pub spicy_lyrics: SpicyConfig,
    pub lrc_red: SearchProviderConfig,
    pub unison: SearchProviderConfig,
    pub lrcmux: SearchProviderConfig,
    pub lrclib: SearchProviderConfig,
    pub prefer_synced: bool,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            order: vec![
                "local-library".into(),
                "spicy-lyrics".into(),
                "lrc-red".into(),
                "unison".into(),
                "lrcmux".into(),
                "lrclib".into(),
            ],
            disabled: Vec::new(),
            local_library: LocalLibraryConfig::default(),
            spicy_lyrics: SpicyConfig::default(),
            lrc_red: SearchProviderConfig::default(),
            unison: SearchProviderConfig::default(),
            lrcmux: SearchProviderConfig::default(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LocalLibraryConfig {
    pub duration_tolerance_secs: f64,
    pub default_dir: String,
    pub prioritize: bool,
}

impl Default for LocalLibraryConfig {
    fn default() -> Self {
        Self {
            duration_tolerance_secs: 2.0,
            default_dir: String::new(),
            prioritize: true,
        }
    }
}

impl LocalLibraryConfig {
    pub fn import_start(&self) -> Option<String> {
        let dir = self.default_dir.trim();
        if dir.is_empty() || !crate::library::expand_path(dir).is_dir() {
            return None;
        }
        Some(format!("{}/", dir.trim_end_matches('/')))
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
    pub check_results: bool,
    pub checked_results: u32,
}

impl Default for SpicyConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            min_tier: MatchTier::Exact,
            check_results: true,
            checked_results: 5,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyllableSpacing {
    #[default]
    Joined,
    Separate,
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
    pub syllable_spacing: SyllableSpacing,
    pub mouse: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            renderer: Renderer::Pixel,
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
            font_scale: 2.0,
            glow: false,
            emphasis: true,
            syllable_spacing: SyllableSpacing::Joined,
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
    pub fix_background_lines: bool,
    pub interludes: bool,
    pub interlude_threshold_secs: f64,
    pub reserve_interlude_space: bool,
    pub join_split_words: bool,
    pub resume_follow_secs: f64,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        Self {
            romanization: false,
            translation: false,
            background_vocals: true,
            fix_background_lines: false,
            interludes: true,
            interlude_threshold_secs: 3.0,
            reserve_interlude_space: false,
            join_split_words: false,
            resume_follow_secs: 5.0,
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
    pub remember_settings: bool,
    pub remember_settings_secs: u32,
}

impl Default for ControlsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            seek_step_secs: 5,
            remember_settings: true,
            remember_settings_secs: 30,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    SpicyLyrics,
    Lyrminal,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Preset::SpicyLyrics, Preset::Lyrminal];

    pub fn label(self) -> &'static str {
        match self {
            Preset::SpicyLyrics => "Spicy Lyrics",
            Preset::Lyrminal => "Lyrminal",
        }
    }

    pub fn apply(self, cfg: &mut Config) {
        let mut d = Config::default();
        if self == Preset::SpicyLyrics {
            d.appearance.font_scale = 2.2;
            d.appearance.glow = true;
            d.appearance.syllable_spacing = SyllableSpacing::Separate;
            d.scroll.animation = ScrollAnimation::Smooth;
            d.scroll.lead_ms = 0;
        }
        let a = &mut cfg.appearance;
        let da = d.appearance;
        a.renderer = da.renderer;
        a.style = da.style;
        a.album_theming = da.album_theming;
        a.background = da.background;
        a.album_art = da.album_art;
        a.show_header = da.show_header;
        a.alignment = da.alignment;
        a.line_gap = da.line_gap;
        a.depth_dimming = da.depth_dimming;
        a.fade_previous = da.fade_previous;
        a.contrast = da.contrast;
        a.font_weight = da.font_weight;
        a.font_scale = da.font_scale;
        a.glow = da.glow;
        a.emphasis = da.emphasis;
        a.syllable_spacing = da.syllable_spacing;
        cfg.scroll.animation = d.scroll.animation;
        cfg.scroll.speed = d.scroll.speed;
        cfg.scroll.lead_ms = d.scroll.lead_ms;
        cfg.lyrics.background_vocals = d.lyrics.background_vocals;
        cfg.lyrics.interludes = d.lyrics.interludes;
        cfg.lyrics.interlude_threshold_secs = d.lyrics.interlude_threshold_secs;
        cfg.lyrics.reserve_interlude_space = d.lyrics.reserve_interlude_space;
        cfg.lyrics.join_split_words = d.lyrics.join_split_words;
    }

    pub fn detect(cfg: &Config, pixel: bool) -> Option<Preset> {
        let mut cfg = cfg.clone();
        if !pixel {
            cfg.appearance.renderer = Config::default().appearance.renderer;
        }
        let current = serde_json::to_value(&cfg).ok()?;
        Preset::ALL.into_iter().find(|p| {
            let mut c = cfg.clone();
            p.apply(&mut c);
            serde_json::to_value(&c).ok().as_ref() == Some(&current)
        })
    }
}

static BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_base_dir(dir: PathBuf) {
    let _ = BASE_DIR.set(dir);
}

pub fn base_dir() -> Option<&'static Path> {
    BASE_DIR.get().map(PathBuf::as_path)
}

fn user_dir(base_rel: &str, system: Option<PathBuf>) -> PathBuf {
    match base_dir() {
        Some(base) => base.join(base_rel),
        None => system.unwrap_or_else(|| PathBuf::from(".")),
    }
    .join("lyrminal")
}

pub fn config_dir() -> PathBuf {
    user_dir(".config", dirs::config_dir())
}

pub fn cache_dir() -> PathBuf {
    user_dir(".cache", dirs::cache_dir().or_else(|| Some(std::env::temp_dir())))
}

pub fn data_dir() -> PathBuf {
    user_dir(".local/share", dirs::data_dir())
}

pub fn state_dir() -> PathBuf {
    user_dir(".local/state", dirs::state_dir().or_else(dirs::data_local_dir))
}

pub fn prepare_base_dir(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let dir = fs::canonicalize(dir).with_context(|| format!("opening {}", dir.display()))?;
    if !dir.is_dir() {
        anyhow::bail!("{} is not a directory", dir.display());
    }
    Ok(dir)
}

pub fn create_temp_base_dir() -> Result<PathBuf> {
    use std::hash::{BuildHasher, Hasher};
    let root = std::env::temp_dir();
    for _ in 0..16 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_u32(std::process::id());
        let dir = root.join(format!("lyrminal-{:012x}", h.finish() & 0xffff_ffff_ffff));
        match fs::create_dir(&dir) {
            Ok(()) => return prepare_base_dir(&dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", dir.display())),
        }
    }
    anyhow::bail!("could not create a temporary base directory in {}", root.display())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_lyrminal_preset() {
        assert_eq!(Preset::detect(&Config::default(), true), Some(Preset::Lyrminal));
    }

    #[test]
    fn creates_missing_and_temporary_base_directories() {
        let root = std::env::temp_dir().join(format!("lyrminal-base-test-{}", std::process::id()));
        let nested = root.join("a/b");
        let made = prepare_base_dir(&nested).unwrap();
        assert!(made.is_dir() && made.is_absolute());
        let temp = create_temp_base_dir().unwrap();
        assert!(temp.is_dir());
        assert!(temp.file_name().unwrap().to_string_lossy().starts_with("lyrminal-"));
        let other = create_temp_base_dir().unwrap();
        assert_ne!(temp, other);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir(&temp);
        let _ = fs::remove_dir(&other);
    }

    #[test]
    fn presets_ignore_the_renderer_without_pixel_support() {
        let mut cfg = Config::default();
        cfg.appearance.renderer = Renderer::Cell;
        assert_eq!(Preset::detect(&cfg, true), None);
        assert_eq!(Preset::detect(&cfg, false), Some(Preset::Lyrminal));
    }

    #[test]
    fn presets_apply_and_are_detected() {
        let mut cfg = Config::default();
        cfg.providers.spicy_lyrics.api_key = "sl_sk_kept".into();
        cfg.sync.offset_ms = 120;
        Preset::SpicyLyrics.apply(&mut cfg);
        assert_eq!(Preset::detect(&cfg, true), Some(Preset::SpicyLyrics));
        assert_eq!(cfg.appearance.renderer, Renderer::Pixel);
        assert_eq!(cfg.appearance.syllable_spacing, SyllableSpacing::Separate);
        assert_eq!(cfg.scroll.animation, ScrollAnimation::Smooth);
        assert_eq!(cfg.scroll.lead_ms, 0);
        assert_eq!(cfg.appearance.font_scale, 2.2);
        assert!(cfg.appearance.glow);
        assert_eq!(cfg.providers.spicy_lyrics.api_key, "sl_sk_kept");
        assert_eq!(cfg.sync.offset_ms, 120);
        cfg.appearance.style = VisualStyle::Bloom;
        assert_eq!(Preset::detect(&cfg, true), None);
        Preset::Lyrminal.apply(&mut cfg);
        assert_eq!(Preset::detect(&cfg, true), Some(Preset::Lyrminal));
    }
}
