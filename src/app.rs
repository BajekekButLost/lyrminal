use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{Config, MatchTier, PlayerMode, Renderer, VisualStyle};
use crate::library::{self, Entry, Library, Source};
use crate::lyrics::process::{self, ItemKind, PrepareOptions, Prepared};
use crate::lyrics::{Lyrics, SyncKind};
use crate::player::clock::PlaybackClock;
use crate::player::{MediaKind, PlayerCommand, PlayerEvent, PlayerInfo, Snapshot, Status, TrackMeta};
use crate::providers::cache::Cache;
use crate::providers::resolve::{self, Attempt, Outcome, Resolution};
use crate::providers::{Candidate, RateLimit, Registry, TrackQuery, ordered_ids};
use crate::render::{FrameInfo, Follow};
use crate::render::cell::CellRenderer;
use crate::render::pixel::{Fonts, PixelRenderer};
use crate::state::{OnboardingStatus, State};
use crate::term::TermCaps;
use crate::theme::{Artwork, Theme};
use crate::ui::draw::{Target, UiHits};
use crate::ui::keys::{self, Action};
use crate::ui::library::{self as library_ui, ImportRequest, LibraryDialog};
use crate::ui::onboarding::{self, Onboarding, Row as GuideRow, Step};
use crate::ui::settings::{self, Kind, SettingsAction, SettingsOutcome, SettingsState};
use crate::update::{self, Release, Status as UpdateStatus};

const FETCH_DEBOUNCE: Duration = Duration::from_millis(350);
const TOAST_FOR: Duration = Duration::from_secs(3);
const MOUSE_SCROLL_ROWS: f64 = 4.0;
const CONFIG_CHECK_EVERY: Duration = Duration::from_secs(1);

pub enum LyricsState {
    NoPlayer,
    NoTrack,
    Unsupported(&'static str),
    Waiting,
    Loading,
    Ready {
        resolution: Box<Resolution>,
        prepared: Arc<Prepared>,
        id: usize,
    },
    NotFound(Vec<Attempt>),
}

pub enum SyncRow {
    Auto,
    Header { provider: String, status: Option<String> },
    Candidate(Candidate),
}

pub struct SyncPicker {
    pub generation: u64,
    pub pending: Vec<String>,
    pub results: Vec<(String, Result<Vec<Candidate>, String>)>,
    pub selected: usize,
    pub applying: Option<Candidate>,
    pub error: Option<String>,
}

impl SyncPicker {
    pub fn rows(&self, order: &[String]) -> Vec<SyncRow> {
        let mut rows = vec![SyncRow::Auto];
        for id in order {
            let loading = self.pending.iter().any(|p| p == id);
            let result = self.results.iter().find(|(p, _)| p == id).map(|(_, r)| r);
            if !loading && result.is_none() {
                continue;
            }
            let status = if loading {
                Some("searching…".to_string())
            } else {
                match result {
                    Some(Ok(c)) if c.is_empty() => Some("no results".into()),
                    Some(Err(e)) => Some(e.clone()),
                    _ => None,
                }
            };
            rows.push(SyncRow::Header {
                provider: id.clone(),
                status,
            });
            if let Some(Ok(cands)) = result {
                rows.extend(cands.iter().cloned().map(SyncRow::Candidate));
            }
        }
        rows
    }

    pub fn group_starts(rows: &[SyncRow]) -> Vec<usize> {
        let mut starts = Vec::new();
        let mut open = true;
        for (i, row) in rows.iter().enumerate() {
            match row {
                SyncRow::Header { .. } => open = true,
                _ if open => {
                    starts.push(i);
                    open = false;
                }
                _ => {}
            }
        }
        starts
    }
}

pub enum Overlay {
    None,
    Help,
    Settings(SettingsState),
    Players { selected: usize },
    Syncs(SyncPicker),
    Library(LibraryDialog),
}

pub const TEMPORARY_ID: &str = "temporary";

pub enum LibraryDone {
    Imported {
        identity: String,
        key: String,
        resolution: Resolution,
        temporary: bool,
        warning: Option<String>,
    },
    Applied {
        identity: String,
        key: String,
        resolution: Resolution,
    },
    Removed(Entry),
}

pub enum AppMsg {
    Library { seq: u64, result: Result<LibraryDone, String> },
    Resolved { generation: u64, outcome: Outcome },
    Artwork { url: String, result: Result<Artwork, String> },
    Search { generation: u64, provider: String, result: Result<Vec<Candidate>, String> },
    Fetched { generation: u64, candidate: Candidate, result: Result<Lyrics, String> },
    RateLimited(RateLimit),
    CacheCleared(Result<(), String>),
    Fonts(Box<Fonts>),
    UpdateChecked { manual: bool, result: Result<Option<Release>, String> },
    UpdateProgress { done: u64, total: u64 },
    UpdateInstalled(Result<std::path::PathBuf, String>),
}

pub struct App {
    pub cfg: Config,
    pub state: State,
    pub caps: TermCaps,
    pub client: reqwest::Client,
    pub registry: Registry,
    pub cache: Cache,
    pub library: Library,
    pub temporary: Option<(String, Resolution)>,
    library_seq: u64,
    player_tx: UnboundedSender<PlayerCommand>,
    msg_tx: UnboundedSender<AppMsg>,
    pub players: Vec<PlayerInfo>,
    pub snapshot: Option<Snapshot>,
    pub player_error: Option<String>,
    pub clock: PlaybackClock,
    pub track: Option<TrackMeta>,
    track_identity: Option<String>,
    pub lyrics: LyricsState,
    generation: u64,
    picker_seq: u64,
    fetch_due: Option<Instant>,
    bypass_cache: bool,
    pub artwork: Option<Artwork>,
    art_loading: Option<String>,
    pub theme: Theme,
    pub cell: CellRenderer,
    pub pixel: Option<PixelRenderer>,
    fonts_loading: bool,
    pub follow: Follow,
    last_scroll: Instant,
    pub overlay: Overlay,
    settings_left: Option<(SettingsState, Instant)>,
    pub guide: Option<Onboarding>,
    pub toast: Option<(String, Instant)>,
    pub quit: bool,
    start: Instant,
    last_frame: Instant,
    next_prepared_id: usize,
    pub frame: FrameInfo,
    pub dirty: bool,
    pub progress_bar: Option<(u16, u16, u16)>,
    pub scrub: Option<f64>,
    pub ui: UiHits,
    pub update: UpdateStatus,
    update_checks: bool,
    next_update_check: Option<Instant>,
    pub restart_into: Option<std::path::PathBuf>,
    config_stamp: Option<(std::time::SystemTime, u64)>,
    next_config_check: Instant,
    base: Config,
}

impl App {
    pub fn new(
        cfg: Config,
        config_stamp: Option<(std::time::SystemTime, u64)>,
        caps: TermCaps,
        player_tx: UnboundedSender<PlayerCommand>,
        msg_tx: UnboundedSender<AppMsg>,
    ) -> Self {
        let base = cfg.clone();
        let mut cfg = cfg;
        crate::overrides::apply(&mut cfg);
        let client = crate::providers::http_client();
        let registry = Registry::from_config(&cfg, &client);
        let cache = Cache::new(crate::config::cache_dir(), cfg.cache.ttl_days, cfg.cache.enabled);
        let theme = Theme::build(&cfg.appearance, None, caps.background);
        let mut clock = PlaybackClock::default();
        clock.smoothing = cfg.sync.smoothing;
        clock.snap_threshold = cfg.sync.snap_threshold_ms as f64 / 1000.0;
        let now = Instant::now();
        let mut app = Self {
            cfg,
            state: State::load(),
            caps,
            client,
            registry,
            cache,
            library: Library::default(),
            temporary: None,
            library_seq: 0,
            player_tx,
            msg_tx,
            players: Vec::new(),
            snapshot: None,
            player_error: None,
            clock,
            track: None,
            track_identity: None,
            lyrics: LyricsState::NoPlayer,
            generation: 0,
            picker_seq: 0,
            fetch_due: None,
            bypass_cache: false,
            artwork: None,
            art_loading: None,
            theme,
            cell: CellRenderer::default(),
            pixel: None,
            fonts_loading: false,
            follow: Follow::default(),
            last_scroll: now,
            overlay: Overlay::None,
            settings_left: None,
            guide: None,
            toast: None,
            quit: false,
            start: now,
            last_frame: now,
            next_prepared_id: 1,
            frame: FrameInfo::default(),
            dirty: true,
            progress_bar: None,
            scrub: None,
            ui: UiHits::default(),
            update: UpdateStatus::Idle,
            update_checks: false,
            next_update_check: None,
            restart_into: None,
            config_stamp,
            next_config_check: now + CONFIG_CHECK_EVERY,
            base,
        };
        app.ensure_fonts();
        app
    }

    pub fn pixel_supported(&self) -> bool {
        self.caps.kitty_graphics && self.caps.cell_px.is_some()
    }

    pub fn use_pixel(&self) -> bool {
        self.cfg.appearance.renderer == Renderer::Pixel && self.pixel_supported()
    }

    pub fn pixel_active(&self) -> bool {
        self.use_pixel() && self.pixel.is_some() && matches!(self.lyrics, LyricsState::Ready { .. })
    }

    fn ensure_fonts(&mut self) {
        if !self.use_pixel() || self.pixel.is_some() || self.fonts_loading {
            return;
        }
        self.fonts_loading = true;
        let tx = self.msg_tx.clone();
        tokio::task::spawn_blocking(move || {
            let fonts = crate::render::pixel::load_fonts();
            let _ = tx.send(AppMsg::Fonts(Box::new(fonts)));
        });
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
        self.dirty = true;
    }

    fn notify_rate_limits(&mut self, limits: &[RateLimit]) {
        let built_in = self.cfg.providers.spicy_lyrics.own_key().is_none();
        if let Some(msg) = crate::providers::rate_limit_notice(limits, built_in) {
            self.notify(msg);
        }
    }

    pub fn wall(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    pub fn track_key(&self) -> Option<String> {
        self.track.as_ref().map(|t| t.query().key())
    }

    pub fn track_offset_ms(&self) -> i64 {
        self.track_key().map(|k| self.state.offset_for(&k)).unwrap_or(0)
    }

    pub fn song_time(&mut self, now: Instant) -> f64 {
        let offset = (self.cfg.sync.offset_ms + self.track_offset_ms()) as f64 / 1000.0;
        self.clock.now(now) + offset
    }

    pub fn position(&self, now: Instant) -> f64 {
        self.clock.raw(now)
    }

    pub fn prepared(&self) -> Option<(Arc<Prepared>, usize)> {
        match &self.lyrics {
            LyricsState::Ready { prepared, id, .. } => Some((prepared.clone(), *id)),
            _ => None,
        }
    }

    pub fn begin_frame(&mut self, now: Instant) -> f64 {
        let dt = now.saturating_duration_since(self.last_frame).as_secs_f64().min(0.25);
        self.last_frame = now;
        dt
    }

    fn prepare_options(&self) -> PrepareOptions {
        let fade = self.cfg.appearance.fade_previous;
        PrepareOptions {
            interludes: self.cfg.lyrics.interludes,
            interlude_threshold: if fade {
                self.cfg.lyrics.interlude_threshold_secs.max(5.0)
            } else {
                self.cfg.lyrics.interlude_threshold_secs
            },
            extend_lines: self.cfg.appearance.style == VisualStyle::Bloom || fade,
            join_split_words: self.cfg.lyrics.join_split_words,
            fix_background: self.cfg.lyrics.fix_background_lines,
        }
    }

    fn install(&mut self, resolution: Resolution) {
        let prepared = Arc::new(process::prepare(&resolution.lyrics, self.prepare_options()));
        let id = self.next_prepared_id;
        self.next_prepared_id += 1;
        self.lyrics = LyricsState::Ready {
            resolution: Box::new(resolution),
            prepared,
            id,
        };
        self.cell.reset();
        if let Some(p) = self.pixel.as_mut() {
            p.reset();
        }
        self.dirty = true;
    }

    fn reprepare(&mut self) {
        let resolution = match &self.lyrics {
            LyricsState::Ready { resolution, .. } => Some((**resolution).clone()),
            _ => None,
        };
        if let Some(r) = resolution {
            self.install(r);
        }
    }

    pub fn tick(&mut self, now: Instant) {
        if let Some(due) = self.fetch_due
            && now >= due
        {
            self.fetch_due = None;
            self.start_fetch();
        }
        if let Some((_, at)) = &self.toast
            && at.elapsed() > TOAST_FOR
        {
            self.toast = None;
            self.dirty = true;
        }
        if self.follow.manual
            && !matches!(&self.lyrics, LyricsState::Ready { prepared, .. } if prepared.lyrics.kind == SyncKind::Static)
            && self.last_scroll.elapsed().as_secs_f64() > self.cfg.lyrics.resume_follow_secs
            && matches!(self.overlay, Overlay::None)
        {
            self.follow.manual = false;
            self.dirty = true;
        }
        self.update_tick(now);
        self.guide_tick(now);
        self.watch_config(now);
    }

    pub fn needs_animation(&self) -> bool {
        self.dirty
            || self.frame.animating
            || self.guide.as_ref().is_some_and(|g| g.found_at.is_some())
            || self.cell.is_animating()
            || (self.clock.is_playing() && matches!(self.lyrics, LyricsState::Ready { .. }))
            || matches!(self.lyrics, LyricsState::Loading | LyricsState::Waiting)
            || matches!(&self.overlay, Overlay::Syncs(p) if !p.pending.is_empty() || p.applying.is_some())
            || matches!(&self.overlay, Overlay::Library(d) if d.busy.is_some())
    }

    fn start_fetch(&mut self) {
        let Some(track) = self.track.clone() else { return };
        if let Some((identity, resolution)) = &self.temporary
            && self.track_identity.as_ref() == Some(identity)
        {
            let resolution = resolution.clone();
            self.bypass_cache = false;
            self.install(resolution);
            return;
        }
        let q = track.query();
        if !q.is_searchable() && q.spotify_id.is_none() {
            self.lyrics = LyricsState::NotFound(Vec::new());
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.lyrics = LyricsState::Loading;
        self.dirty = true;
        let registry = self.registry.clone();
        let cache = self.cache.clone();
        let choice = self.state.choices.get(&q.key()).cloned();
        let bypass = std::mem::take(&mut self.bypass_cache);
        let tx = self.msg_tx.clone();
        tokio::spawn(async move {
            let outcome = resolve::resolve(&registry, &cache, &q, choice, bypass).await;
            let _ = tx.send(AppMsg::Resolved { generation, outcome });
        });
    }

    fn schedule_fetch(&mut self, delay: Duration) {
        self.generation += 1;
        self.lyrics = LyricsState::Waiting;
        self.fetch_due = Some(Instant::now() + delay);
        self.dirty = true;
    }

    fn load_artwork(&mut self) {
        let wants = self.cfg.appearance.album_theming || (self.cfg.appearance.album_art && self.caps.kitty_graphics);
        let Some(url) = self.track.as_ref().and_then(|t| t.art_url.clone()) else {
            if self.artwork.is_some() {
                self.artwork = None;
                self.retheme();
            }
            return;
        };
        if !wants
            || self.artwork.as_ref().is_some_and(|a| a.url == url)
            || self.art_loading.as_deref() == Some(url.as_str())
        {
            return;
        }
        self.art_loading = Some(url.clone());
        let client = self.client.clone();
        let tx = self.msg_tx.clone();
        tokio::spawn(async move {
            let result = crate::theme::load_artwork(&client, &url).await.map_err(|e| e.to_string());
            let _ = tx.send(AppMsg::Artwork { url, result });
        });
    }

    pub fn retheme(&mut self) {
        self.theme = Theme::build(
            &self.cfg.appearance,
            self.artwork.as_ref().map(|a| a.palette),
            self.caps.background,
        );
        self.dirty = true;
    }

    fn on_track_change(&mut self, track: Option<TrackMeta>) {
        self.track_identity = track.as_ref().map(|t| t.identity_key());
        self.track = track;
        if self
            .temporary
            .as_ref()
            .is_some_and(|(id, _)| self.track_identity.as_ref() != Some(id))
        {
            self.temporary = None;
        }
        self.follow = Follow::default();
        self.cell.reset();
        self.fetch_due = None;
        self.generation += 1;
        if let Overlay::Syncs(_) = self.overlay {
            self.set_overlay(Overlay::None);
        }
        match self.track.as_ref().map(|t| t.media_kind()) {
            None => self.lyrics = LyricsState::NoTrack,
            Some(MediaKind::Ad) => self.lyrics = LyricsState::Unsupported("Advertisement"),
            Some(MediaKind::Episode) => self.lyrics = LyricsState::Unsupported("Podcast episode"),
            _ => self.schedule_fetch(FETCH_DEBOUNCE),
        }
        self.load_artwork();
        self.dirty = true;
    }

    pub fn on_player(&mut self, ev: PlayerEvent) {
        self.dirty = true;
        match ev {
            PlayerEvent::Players(p) => {
                self.players = p;
                if let Overlay::Players { selected } = &mut self.overlay {
                    *selected = (*selected).min(self.players.len());
                }
            }
            PlayerEvent::Error(e) => self.player_error = Some(e),
            PlayerEvent::Active(None) => {
                self.snapshot = None;
                self.clock.reset(0.0, Instant::now(), false, 1.0);
                if self.track.is_some() || !matches!(self.lyrics, LyricsState::NoPlayer) {
                    self.on_track_change(None);
                }
                self.lyrics = LyricsState::NoPlayer;
            }
            PlayerEvent::Active(Some(s)) => {
                self.player_error = None;
                let identity = s.track.as_ref().map(|t| t.identity_key());
                let changed_player = self.snapshot.as_ref().map(|o| o.player.bus.as_str()) != Some(s.player.bus.as_str());
                let playing = s.status == Status::Playing;
                if identity != self.track_identity || changed_player {
                    self.clock.duration = s.track.as_ref().and_then(|t| t.length);
                    self.clock.reset(s.position, s.sampled_at, playing, s.rate);
                    self.on_track_change(s.track.clone());
                } else {
                    let status_changed = self.snapshot.as_ref().map(|o| o.status) != Some(s.status);
                    self.clock.duration = s.track.as_ref().and_then(|t| t.length);
                    self.clock.set_playing(playing, s.rate, s.sampled_at);
                    self.clock.sample(s.position, s.sampled_at, status_changed);
                    if let Some(t) = &s.track
                        && self.track.as_ref().map(|o| &o.art_url) != Some(&t.art_url)
                    {
                        self.track = Some(t.clone());
                        self.load_artwork();
                    }
                }
                self.snapshot = Some(s);
            }
            PlayerEvent::Position { position, at, seeked } => {
                self.clock.sample(position, at, seeked);
                if seeked {
                    self.follow.manual = false;
                }
            }
        }
    }

    pub fn on_msg(&mut self, msg: AppMsg) {
        self.dirty = true;
        match msg {
            AppMsg::Library { seq, result } => self.on_library(seq, result),
            AppMsg::Resolved { generation, outcome } => {
                if generation != self.generation {
                    return;
                }
                match outcome.result {
                    Some(res) => self.install(res),
                    None => self.lyrics = LyricsState::NotFound(outcome.attempts),
                }
                self.notify_rate_limits(&outcome.rate_limits);
            }
            AppMsg::RateLimited(limit) => self.notify_rate_limits(&[limit]),
            AppMsg::Artwork { url, result } => {
                if self.art_loading.as_deref() == Some(url.as_str()) {
                    self.art_loading = None;
                }
                let current = self.track.as_ref().and_then(|t| t.art_url.as_deref()) == Some(url.as_str());
                match result {
                    Ok(art) if current => {
                        self.artwork = Some(art);
                        self.retheme();
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if current {
                            self.artwork = None;
                            self.retheme();
                            self.player_error = Some(format!("cover art: {e}"));
                        }
                    }
                }
            }
            AppMsg::Search {
                generation,
                provider,
                result,
            } => {
                if let Overlay::Syncs(p) = &mut self.overlay
                    && p.generation == generation
                {
                    p.pending.retain(|x| *x != provider);
                    p.results.retain(|(x, _)| *x != provider);
                    p.results.push((provider, result));
                }
            }
            AppMsg::Fetched {
                generation,
                candidate,
                result,
            } => {
                let Overlay::Syncs(p) = &mut self.overlay else { return };
                if p.generation != generation {
                    return;
                }
                p.applying = None;
                match result {
                    Ok(lyrics) => {
                        if let Some(key) = self.track_key() {
                            self.state.choices.insert(key, candidate.clone());
                            self.state.save();
                        }
                        let name = crate::providers::provider_name(&candidate.provider).to_string();
                        self.set_overlay(Overlay::None);
                        self.temporary = None;
                        self.generation += 1;
                        self.install(Resolution {
                            candidate,
                            lyrics,
                            manual: true,
                        });
                        self.notify(format!("Using the {name} sync for this track"));
                    }
                    Err(e) => p.error = Some(e),
                }
            }
            AppMsg::Fonts(fonts) => {
                self.fonts_loading = false;
                self.pixel = Some(PixelRenderer::new(*fonts));
            }
            AppMsg::CacheCleared(r) => match r {
                Ok(()) => self.notify("Cache cleared"),
                Err(e) => self.notify(format!("Could not clear the cache: {e}")),
            },
            AppMsg::UpdateChecked { manual, result } => self.on_update_checked(manual, result),
            AppMsg::UpdateProgress { done, total } => {
                if let UpdateStatus::Downloading { done: d, total: t, .. } = &mut self.update {
                    *d = done;
                    *t = total;
                }
            }
            AppMsg::UpdateInstalled(result) => {
                let UpdateStatus::Downloading { release, .. } = std::mem::replace(&mut self.update, UpdateStatus::Idle) else {
                    return;
                };
                match result {
                    Ok(path) => {
                        self.notify(format!("Lyrminal v{} is installed · restart Lyrminal or press U", release.version));
                        self.update = UpdateStatus::Installed { release, path };
                    }
                    Err(e) => {
                        self.notify(format!("Update failed: {e}"));
                        self.update = UpdateStatus::Failed(e);
                    }
                }
            }
        }
    }

    pub fn on_ipc(&mut self, msg: crate::ipc::Incoming) {
        let answer = match msg.request {
            crate::ipc::Request::UseEntry { id } => return self.use_entry(&id, msg.reply),
            crate::ipc::Request::NowPlaying => crate::ipc::ok(crate::now_playing::snapshot(self, Instant::now())),
            crate::ipc::Request::ReloadConfig => match self.reload_config() {
                Ok(()) => crate::ipc::ok(serde_json::json!({})),
                Err(e) => crate::ipc::error(e),
            },
            crate::ipc::Request::LibraryChanged(change) => {
                self.library_changed(&change);
                crate::ipc::ok(serde_json::json!({}))
            }
            crate::ipc::Request::Temporary(t) => self.load_temporary(t),
        };
        let _ = msg.reply.send(answer);
    }

    fn use_entry(&mut self, id: &str, reply: tokio::sync::oneshot::Sender<serde_json::Value>) {
        let Some(entry) = self.library.get(id) else {
            let _ = reply.send(crate::ipc::error(format!("there is no sync with the id {id} in the library")));
            return;
        };
        let Some(track) = self.playing_track() else {
            let _ = reply.send(crate::ipc::error("nothing is playing"));
            return;
        };
        let (q, identity) = (track.query(), track.identity_key());
        let client = self.client.clone();
        let lib = self.library.clone();
        let tx = self.msg_tx.clone();
        tokio::spawn(async move {
            match lib.load(&entry, &client, q.duration).await {
                Ok(lyrics) => {
                    let key = q.key();
                    let tier = if entry.key == key { MatchTier::Exact } else { MatchTier::Related };
                    let done = LibraryDone::Applied {
                        identity,
                        key,
                        resolution: Resolution {
                            candidate: crate::providers::local::entry_candidate(&entry, Some(tier)),
                            lyrics,
                            manual: true,
                        },
                    };
                    let _ = tx.send(AppMsg::Library {
                        seq: u64::MAX,
                        result: Ok(done),
                    });
                    let _ = reply.send(crate::ipc::ok(serde_json::json!({ "title": entry.track.title })));
                }
                Err(e) => {
                    let _ = reply.send(crate::ipc::error(e));
                }
            }
        });
    }

    fn playing_track(&self) -> Option<&TrackMeta> {
        self.track.as_ref().filter(|t| !matches!(t.media_kind(), MediaKind::Ad | MediaKind::Episode))
    }

    fn load_temporary(&mut self, t: crate::ipc::TemporaryLyrics) -> serde_json::Value {
        let Some(track) = self.playing_track() else {
            return crate::ipc::error("nothing is playing");
        };
        let req = ImportRequest {
            track: track.query(),
            identity: track.identity_key(),
            temporary: true,
            source: t.source,
            watch: false,
            format: t.format,
            pasted: None,
        };
        match temporary_import(req, &t.text) {
            Ok(LibraryDone::Imported {
                identity,
                resolution,
                warning,
                ..
            }) => {
                let title = resolution.candidate.title.clone();
                self.temporary = Some((identity, resolution.clone()));
                self.show_now(resolution);
                self.notify("Loaded temporary lyrics for this song");
                crate::ipc::ok(serde_json::json!({ "title": title, "warning": warning }))
            }
            Ok(_) => crate::ipc::error("the lyrics could not be loaded"),
            Err(e) => crate::ipc::error(e),
        }
    }

    fn library_changed(&mut self, change: &crate::ipc::LibraryChange) {
        if change.forget_choices(&mut self.state.choices) {
            self.state.save();
        }
        let entries = self.library.entries();
        if let Overlay::Library(d) = &mut self.overlay {
            d.set_entries(entries);
        }
        let local = crate::providers::local::ID;
        let showing = matches!(&self.lyrics, LyricsState::Ready { resolution, .. }
            if resolution.candidate.provider == local
                && (change.changed.contains(&resolution.candidate.id) || change.removed.contains(&resolution.candidate.id)));
        let added = self.track_key().is_some_and(|k| change.added.contains(&k));
        let touched = !(change.added.is_empty() && change.changed.is_empty() && change.removed.is_empty());
        let loading = matches!(self.lyrics, LyricsState::Loading) && touched;
        if self.track.is_some() && self.temporary.is_none() && (showing || added || loading) {
            self.schedule_fetch(Duration::ZERO);
        }
        self.dirty = true;
    }

    pub fn reload_config(&mut self) -> Result<(), String> {
        let stamp = Config::stamp();
        let base = Config::load().map_err(|e| format!("{e:#}"))?;
        self.config_stamp = stamp;
        let mut cfg = base.clone();
        crate::overrides::apply(&mut cfg);
        self.base = base;
        let old = std::mem::replace(&mut self.cfg, cfg);
        self.apply_changes(&old);
        self.notify("Settings reloaded");
        Ok(())
    }

    pub fn on_event(&mut self, ev: Event) {
        self.dirty = true;
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Paste(text) => match &mut self.overlay {
                Overlay::Settings(s) => settings::paste(s, &text),
                Overlay::Library(d) => library_ui::paste(d, &text),
                Overlay::None => self.guide_paste(&text),
                _ => {}
            },
            Event::Mouse(m) if self.cfg.appearance.mouse && !matches!(self.overlay, Overlay::None) => self.on_overlay_mouse(m),
            Event::Mouse(m) if self.cfg.appearance.mouse => match m.kind {
                MouseEventKind::ScrollDown if self.ui.in_guide(m.column, m.row) => self.guide_press(KeyCode::Down),
                MouseEventKind::ScrollUp if self.ui.in_guide(m.column, m.row) => self.guide_press(KeyCode::Up),
                MouseEventKind::ScrollDown => self.scroll_by(MOUSE_SCROLL_ROWS),
                MouseEventKind::ScrollUp => self.scroll_by(-MOUSE_SCROLL_ROWS),
                MouseEventKind::Down(MouseButton::Left) => self.on_press(m.column, m.row),
                MouseEventKind::Drag(MouseButton::Left) => {
                    if self.scrub.is_some()
                        && let Some(pos) = self.progress_position(m.column)
                    {
                        self.scrub = Some(pos);
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    if let Some(pos) = self.scrub.take() {
                        self.seek_to(pos);
                    }
                }
                _ => {}
            },
            Event::Resize(..) => {
                self.caps.refresh_cell_size();
                self.cell.reset();
                if let Some(p) = self.pixel.as_mut() {
                    p.reset();
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Help => {
                self.set_overlay(Overlay::None);
                return;
            }
            Overlay::Settings(state) => {
                let old = self.cfg.clone();
                let outcome = settings::handle_key(state, &mut self.cfg, key);
                match outcome {
                    SettingsOutcome::None => {}
                    SettingsOutcome::Close => self.set_overlay(Overlay::None),
                    SettingsOutcome::Changed => self.apply_config(&old),
                    SettingsOutcome::Action(a) => self.settings_action(a),
                    SettingsOutcome::Locked(m) => self.notify(m),
                }
                return;
            }
            Overlay::Players { selected } => {
                let n = self.players.len() + 1;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('p') => self.set_overlay(Overlay::None),
                    KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(n - 1),
                    KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        let choice = *selected;
                        let old = self.cfg.clone();
                        if choice == 0 {
                            self.cfg.player.mode = PlayerMode::Auto;
                        } else if let Some(p) = self.players.get(choice - 1) {
                            self.cfg.player.mode = PlayerMode::Fixed;
                            self.cfg.player.target = p.process.clone().unwrap_or_else(|| p.short_bus().to_string());
                        }
                        self.set_overlay(Overlay::None);
                        self.apply_config(&old);
                    }
                    _ => {}
                }
                return;
            }
            Overlay::Library(dialog) => {
                dialog.columns = self.ui.columns.max(1);
                match library_ui::handle_key(dialog, key) {
                    library_ui::Outcome::None => {}
                    library_ui::Outcome::Close => self.close_library(),
                    library_ui::Outcome::Import(req) => self.start_import(req),
                    library_ui::Outcome::Apply(entry) => self.apply_entry(entry),
                    library_ui::Outcome::Remove(entry) => self.remove_entry(entry),
                }
                return;
            }
            Overlay::Syncs(picker) => {
                let order = ordered_ids(&self.cfg);
                let rows = picker.rows(&order);
                let selectable = |r: &SyncRow| !matches!(r, SyncRow::Header { .. });
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => self.set_overlay(Overlay::None),
                    KeyCode::Down | KeyCode::Char('j') => {
                        let mut i = picker.selected + 1;
                        while i < rows.len() && !selectable(&rows[i]) {
                            i += 1;
                        }
                        if i < rows.len() {
                            picker.selected = i;
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        let mut i = picker.selected;
                        while i > 0 {
                            i -= 1;
                            if selectable(&rows[i]) {
                                picker.selected = i;
                                break;
                            }
                        }
                    }
                    KeyCode::Tab | KeyCode::BackTab => {
                        let dir = if key.code == KeyCode::Tab { 1 } else { -1 };
                        if let Some(i) = crate::ui::jump_group(&SyncPicker::group_starts(&rows), picker.selected, dir) {
                            picker.selected = i;
                        }
                    }
                    KeyCode::Char('r') => self.open_syncs(),
                    KeyCode::Enter => match rows.get(picker.selected) {
                        Some(SyncRow::Auto) => {
                            if let Some(key) = self.track_key() {
                                self.state.choices.remove(&key);
                                self.state.save();
                            }
                            self.set_overlay(Overlay::None);
                            self.temporary = None;
                            self.schedule_fetch(Duration::ZERO);
                            self.notify("Picking the best sync automatically");
                        }
                        Some(SyncRow::Candidate(c)) => {
                            let c = c.clone();
                            picker.applying = Some(c.clone());
                            picker.error = None;
                            let generation = picker.generation;
                            let registry = self.registry.clone();
                            let cache = self.cache.clone();
                            let tx = self.msg_tx.clone();
                            tokio::spawn(async move {
                                let result = resolve::fetch_candidate(&registry, &cache, &c, false).await.map_err(|e| {
                                    if let Some(limit) = e.rate_limit(crate::providers::provider_name(&c.provider)) {
                                        let _ = tx.send(AppMsg::RateLimited(limit));
                                    }
                                    e.to_string()
                                });
                                let _ = tx.send(AppMsg::Fetched {
                                    generation,
                                    candidate: c,
                                    result,
                                });
                            });
                        }
                        _ => {}
                    },
                    _ => {}
                }
                return;
            }
        }
        if self.on_guide_key(key) {
            return;
        }
        if let Some(action) = keys::map(key) {
            self.on_action(action);
        }
    }

    fn press(&mut self, code: KeyCode) {
        self.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn set_overlay(&mut self, next: Overlay) {
        if let Overlay::Settings(state) = std::mem::replace(&mut self.overlay, next) {
            self.settings_left = Some((state, Instant::now()));
        }
    }

    fn open_settings(&mut self) {
        let state = SettingsState::reopen(self.settings_left.take(), &self.cfg, self.pixel_supported(), Instant::now());
        self.set_overlay(Overlay::Settings(state));
    }

    fn on_overlay_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollDown if !matches!(self.overlay, Overlay::Help) => self.press(KeyCode::Down),
            MouseEventKind::ScrollUp if !matches!(self.overlay, Overlay::Help) => self.press(KeyCode::Up),
            MouseEventKind::Down(MouseButton::Left) => self.on_overlay_click(m.column, m.row),
            _ => {}
        }
    }

    fn on_overlay_click(&mut self, column: u16, row: u16) {
        if !self.ui.in_modal(column, row) {
            self.close_library();
            return;
        }
        let target = self.ui.at(column, row);
        let key = match (&mut self.overlay, target) {
            (Overlay::Help, _) => {
                self.set_overlay(Overlay::None);
                None
            }
            (Overlay::Settings(s), Some(Target::Section(i))) => {
                s.editing = None;
                s.confirm = None;
                s.page = None;
                s.section = i;
                s.item = 0;
                None
            }
            (Overlay::Settings(s), Some(Target::Back)) => {
                s.editing = None;
                s.confirm = None;
                Some(KeyCode::Esc)
            }
            (Overlay::Settings(s), Some(Target::Row(i))) => {
                let same = s.item == i;
                if s.editing.is_some() && same {
                    None
                } else if s.confirm.is_some() {
                    if same {
                        Some(KeyCode::Char('y'))
                    } else {
                        s.confirm = None;
                        s.item = i;
                        None
                    }
                } else {
                    s.editing = None;
                    s.item = i;
                    same.then_some(KeyCode::Enter)
                }
            }
            (Overlay::Settings(_), Some(Target::Decrease)) => Some(KeyCode::Left),
            (Overlay::Settings(_), Some(Target::Increase)) => Some(KeyCode::Right),
            (Overlay::Players { selected }, Some(Target::Row(i))) => {
                let same = *selected == i;
                *selected = i;
                same.then_some(KeyCode::Enter)
            }
            (Overlay::Syncs(p), Some(Target::Row(i))) => {
                let same = p.selected == i;
                p.selected = i;
                same.then_some(KeyCode::Enter)
            }
            (Overlay::Library(d), Some(Target::Row(i))) => {
                let same = d.selected == i;
                if d.editing.is_some() && same {
                    None
                } else if d.confirm_remove.is_some() {
                    if same {
                        Some(KeyCode::Char('y'))
                    } else {
                        d.confirm_remove = None;
                        d.selected = i;
                        None
                    }
                } else {
                    if !same && d.editing.is_some() {
                        library_ui::handle_key(d, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                    }
                    d.selected = i;
                    same.then_some(KeyCode::Enter)
                }
            }
            (Overlay::Library(d), Some(Target::Back)) => {
                d.open_library();
                None
            }
            (Overlay::Library(_), Some(Target::Decrease)) => Some(KeyCode::Left),
            (Overlay::Library(_), Some(Target::Increase)) => Some(KeyCode::Right),
            _ => None,
        };
        if let Some(code) = key {
            self.press(code);
        }
    }

    fn settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::PickPlayer => self.open_players(),
            SettingsAction::Onboarding => self.start_onboarding(),
            SettingsAction::ClearCache => {
                let cache = self.cache.clone();
                let tx = self.msg_tx.clone();
                tokio::spawn(async move {
                    let r = tokio::task::spawn_blocking(move || cache.clear())
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()));
                    let _ = tx.send(AppMsg::CacheCleared(r));
                });
            }
            SettingsAction::ResetDefaults => {
                let old = self.cfg.clone();
                self.cfg = Config::default();
                self.apply_config(&old);
                self.notify("Settings and API keys reset to defaults");
            }
            SettingsAction::Update => self.update_action(),
        }
    }

    pub fn enable_update_checks(&mut self) {
        self.update_checks = true;
        self.next_update_check = None;
    }

    fn update_tick(&mut self, now: Instant) {
        if !self.update_checks
            || !self.cfg.updates.check
            || !matches!(self.update, UpdateStatus::Idle | UpdateStatus::UpToDate | UpdateStatus::Failed(_))
            || self.next_update_check.is_some_and(|at| now < at)
        {
            return;
        }
        self.next_update_check = Some(now + update::CHECK_EVERY);
        self.check_updates(false);
    }

    fn check_updates(&mut self, manual: bool) {
        if self.update.busy() {
            return;
        }
        self.update = UpdateStatus::Checking;
        self.dirty = true;
        let client = self.client.clone();
        let tx = self.msg_tx.clone();
        tokio::spawn(async move {
            let result = update::check(&client).await;
            let _ = tx.send(AppMsg::UpdateChecked { manual, result });
        });
    }

    fn on_update_checked(&mut self, manual: bool, result: Result<Option<Release>, String>) {
        match result {
            Ok(Some(release)) => {
                let installable = release.asset.is_some();
                let version = release.version.clone();
                self.update = UpdateStatus::Available(release);
                if installable && self.cfg.updates.auto_install {
                    self.notify(format!("Installing Lyrminal v{version}"));
                    self.install_update();
                } else if installable {
                    self.notify(format!("Lyrminal v{version} is available · U to update"));
                } else {
                    self.notify(format!("Lyrminal v{version} is available, but there is no prebuilt binary for this system"));
                }
            }
            Ok(None) => {
                self.update = UpdateStatus::UpToDate;
                if manual {
                    self.notify(format!("Lyrminal v{} is up to date", update::CURRENT));
                }
            }
            Err(e) => {
                if manual {
                    self.notify(format!("Could not check for updates: {e}"));
                }
                self.update = UpdateStatus::Failed(e);
            }
        }
    }

    fn install_update(&mut self) {
        let UpdateStatus::Available(release) = &self.update else { return };
        if release.asset.is_none() {
            self.notify("There is no prebuilt binary for this system: rerun install.sh with --from-source");
            return;
        }
        let release = release.clone();
        self.update = UpdateStatus::Downloading {
            release: release.clone(),
            done: 0,
            total: release.asset.as_ref().map(|a| a.size).unwrap_or(0),
        };
        self.dirty = true;
        let client = self.client.clone();
        let tx = self.msg_tx.clone();
        tokio::spawn(async move {
            let mut shown = None;
            let progress_tx = tx.clone();
            let result = update::install(&client, &release, move |done, total| {
                let pct = (total > 0).then(|| done * 100 / total);
                if pct != shown {
                    shown = pct;
                    let _ = progress_tx.send(AppMsg::UpdateProgress { done, total });
                }
            })
            .await;
            let _ = tx.send(AppMsg::UpdateInstalled(result));
        });
    }

    fn update_action(&mut self) {
        match &self.update {
            UpdateStatus::Idle | UpdateStatus::UpToDate | UpdateStatus::Failed(_) => self.check_updates(true),
            UpdateStatus::Available(_) => self.install_update(),
            UpdateStatus::Checking | UpdateStatus::Downloading { .. } => {}
            UpdateStatus::Installed { path, .. } => {
                self.restart_into = Some(path.clone());
                self.quit = true;
            }
        }
    }

    fn open_updates(&mut self) {
        let secs = settings::sections(&self.cfg, self.pixel_supported());
        let Some(section) = secs.iter().position(|s| s.title == settings::UPDATES_SECTION) else { return };
        let item = secs[section].items.iter().position(|i| i.label == settings::UPDATE_LABEL).unwrap_or(0);
        self.open_settings();
        if let Overlay::Settings(s) = &mut self.overlay {
            s.page = None;
            s.editing = None;
            s.confirm = None;
            s.section = section;
            s.item = item;
        }
    }

    pub fn apply_config(&mut self, old: &Config) {
        let json = |v: &dyn erased::Json| v.json();
        let wanted = json(&self.cfg);
        crate::overrides::apply(&mut self.cfg);
        if json(&self.cfg) != wanted {
            self.notify("That setting is locked by a launch flag for this session");
        }
        let file = crate::overrides::persisted(&self.cfg, &self.base);
        if let Err(e) = file.save() {
            self.notify(format!("Could not save settings: {e}"));
        }
        self.base = file;
        self.config_stamp = Config::stamp();
        self.apply_changes(old);
    }

    fn watch_config(&mut self, now: Instant) {
        if !self.cfg.general.live_reload || now < self.next_config_check {
            return;
        }
        self.next_config_check = now + CONFIG_CHECK_EVERY;
        let stamp = Config::stamp();
        if stamp == self.config_stamp {
            return;
        }
        self.config_stamp = stamp;
        if stamp.is_some()
            && let Err(e) = self.reload_config()
        {
            self.notify(format!("The config file has a mistake, keeping the current settings: {e}"));
        }
    }

    fn apply_changes(&mut self, old: &Config) {
        let json = |v: &dyn erased::Json| v.json();
        let providers_changed = json(&self.cfg.providers) != json(&old.providers);
        if providers_changed {
            self.registry = Registry::from_config(&self.cfg, &self.client);
            if matches!(self.lyrics, LyricsState::NotFound(_) | LyricsState::Ready { .. }) && self.track.is_some() {
                let manual = matches!(&self.lyrics, LyricsState::Ready { resolution, .. } if resolution.manual);
                let order_changed = ordered_ids(&self.cfg) != ordered_ids(old) || self.cfg.providers.disabled != old.providers.disabled;
                let key_changed = self.cfg.providers.spicy_lyrics.effective_key() != old.providers.spicy_lyrics.effective_key();
                if !manual && (order_changed || key_changed || matches!(self.lyrics, LyricsState::NotFound(_))) {
                    self.schedule_fetch(Duration::from_millis(600));
                }
            }
        }
        if json(&self.cfg.player) != json(&old.player) {
            let _ = self.player_tx.send(PlayerCommand::Reconfigure(self.cfg.player.clone()));
        }
        if self.cfg.updates.check && !old.updates.check {
            self.next_update_check = None;
        }
        if self.cfg.updates.auto_install && !old.updates.auto_install {
            self.install_update();
        }
        if json(&self.cfg.cache) != json(&old.cache) {
            self.cache = Cache::new(crate::config::cache_dir(), self.cfg.cache.ttl_days, self.cfg.cache.enabled);
        }
        if json(&self.cfg.sync) != json(&old.sync) {
            self.clock.smoothing = self.cfg.sync.smoothing;
            self.clock.snap_threshold = self.cfg.sync.snap_threshold_ms as f64 / 1000.0;
        }
        let appearance_changed = json(&self.cfg.appearance) != json(&old.appearance);
        let lyrics_changed = json(&self.cfg.lyrics) != json(&old.lyrics);
        if appearance_changed {
            self.load_artwork();
            self.retheme();
            self.ensure_fonts();
        }
        let prep_changed = self.cfg.appearance.style != old.appearance.style
            || self.cfg.appearance.fade_previous != old.appearance.fade_previous
            || self.cfg.lyrics.interludes != old.lyrics.interludes
            || self.cfg.lyrics.interlude_threshold_secs != old.lyrics.interlude_threshold_secs
            || self.cfg.lyrics.join_split_words != old.lyrics.join_split_words
            || self.cfg.lyrics.fix_background_lines != old.lyrics.fix_background_lines;
        if prep_changed {
            self.reprepare();
        } else if appearance_changed || lyrics_changed {
            self.cell.reset();
        }
        self.dirty = true;
    }

    fn open_players(&mut self) {
        let current = self.snapshot.as_ref().map(|s| s.player.bus.clone());
        let selected = match self.cfg.player.mode {
            PlayerMode::Auto => 0,
            PlayerMode::Fixed => current
                .and_then(|c| self.players.iter().position(|p| p.bus == c))
                .map(|i| i + 1)
                .unwrap_or(0),
        };
        self.set_overlay(Overlay::Players { selected });
    }

    fn open_syncs(&mut self) {
        let Some(track) = &self.track else {
            self.notify("Nothing is playing");
            return;
        };
        let q: TrackQuery = track.query();
        self.picker_seq += 1;
        let generation = self.picker_seq;
        let pending: Vec<String> = self.registry.providers.iter().map(|p| p.id().to_string()).collect();
        self.set_overlay(Overlay::Syncs(SyncPicker {
            generation,
            pending,
            results: Vec::new(),
            selected: 0,
            applying: None,
            error: None,
        }));
        for p in self.registry.providers.clone() {
            let q = q.clone();
            let tx = self.msg_tx.clone();
            tokio::spawn(async move {
                let result = match p.ready() {
                    Ok(()) => p.search(&q).await.map_err(|e| {
                        if let Some(limit) = e.rate_limit(p.name()) {
                            let _ = tx.send(AppMsg::RateLimited(limit));
                        }
                        e.to_string()
                    }),
                    Err(e) => Err(e),
                };
                let _ = tx.send(AppMsg::Search {
                    generation,
                    provider: p.id().to_string(),
                    result,
                });
            });
        }
    }

    fn open_library(&mut self) {
        let track = self
            .track
            .as_ref()
            .filter(|t| !matches!(t.media_kind(), MediaKind::Ad | MediaKind::Episode))
            .map(|t| (t.query(), t.identity_key()));
        self.library_seq += 1;
        let entries = self.library.entries();
        let start = self.cfg.providers.local_library.import_start();
        self.set_overlay(Overlay::Library(LibraryDialog::new(track, entries, self.library_seq, start)));
    }

    fn close_library(&mut self) {
        self.set_overlay(Overlay::None);
    }

    fn library_busy(&mut self, what: &str) -> Option<u64> {
        let Overlay::Library(d) = &mut self.overlay else { return None };
        d.busy = Some(what.to_string());
        d.error = None;
        d.notice = None;
        Some(d.seq)
    }

    fn spawn_library<F>(&mut self, what: &str, task: F)
    where
        F: std::future::Future<Output = Result<LibraryDone, String>> + Send + 'static,
    {
        let Some(seq) = self.library_busy(what) else { return };
        let tx = self.msg_tx.clone();
        tokio::spawn(async move {
            let result = task.await;
            let _ = tx.send(AppMsg::Library { seq, result });
        });
    }

    fn start_import(&mut self, req: ImportRequest) {
        let what = match (&req.source, req.pasted.is_some()) {
            (Source::Clipboard, true) => "Reading the pasted lyrics",
            (Source::Clipboard, false) => "Reading the clipboard",
            (Source::File(_), _) => "Reading the file",
            (Source::Url(_), _) => "Downloading",
            (Source::Stdin, _) => "Reading the lyrics",
        };
        let client = self.client.clone();
        let lib = self.library.clone();
        self.spawn_library(what, import(req, client, lib));
    }

    fn apply_entry(&mut self, entry: Entry) {
        let Some((q, identity)) = (match &self.overlay {
            Overlay::Library(d) => d.track.clone(),
            _ => None,
        }) else {
            if let Overlay::Library(d) = &mut self.overlay {
                d.error = Some("Nothing is playing, so there is no song to use these lyrics for".into());
            }
            return;
        };
        let client = self.client.clone();
        let lib = self.library.clone();
        self.spawn_library("Loading", async move {
            let lyrics = lib.load(&entry, &client, q.duration).await?;
            let key = q.key();
            let tier = if entry.key == key { MatchTier::Exact } else { MatchTier::Related };
            Ok(LibraryDone::Applied {
                identity,
                key,
                resolution: Resolution {
                    candidate: crate::providers::local::entry_candidate(&entry, Some(tier)),
                    lyrics,
                    manual: true,
                },
            })
        });
    }

    fn remove_entry(&mut self, entry: Entry) {
        let lib = self.library.clone();
        self.spawn_library("Removing", async move {
            let id = entry.id.clone();
            tokio::task::spawn_blocking(move || lib.remove(&id))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| format!("could not update the library: {e}"))?;
            Ok(LibraryDone::Removed(entry))
        });
    }

    fn show_now(&mut self, resolution: Resolution) {
        self.generation += 1;
        self.fetch_due = None;
        self.install(resolution);
    }

    fn on_library(&mut self, seq: u64, result: Result<LibraryDone, String>) {
        let open = matches!(&self.overlay, Overlay::Library(d) if d.seq == seq);
        if open && let Overlay::Library(d) = &mut self.overlay {
            d.busy = None;
        }
        let done = match result {
            Ok(done) => done,
            Err(e) => {
                match &mut self.overlay {
                    Overlay::Library(d) if open => d.error = Some(e),
                    _ => self.notify(format!("Local library: {e}")),
                }
                return;
            }
        };
        let local_on = self.cfg.providers.is_enabled(crate::providers::local::ID);
        match done {
            LibraryDone::Imported {
                identity,
                key,
                resolution,
                temporary,
                warning,
            } => {
                let current = self.track_identity.as_deref() == Some(identity.as_str());
                let mut msg = if temporary {
                    if current {
                        self.temporary = Some((identity, resolution.clone()));
                        self.show_now(resolution);
                        "Loaded temporary lyrics for this song".to_string()
                    } else {
                        "The song changed, so the temporary lyrics were dropped".to_string()
                    }
                } else {
                    if self.state.choices.remove(&key).is_some() {
                        self.state.save();
                    }
                    if current {
                        self.temporary = None;
                        self.show_now(resolution);
                    }
                    if local_on {
                        "Saved to the local library".to_string()
                    } else {
                        "Saved, but the Local library provider is off: Esc → Providers".to_string()
                    }
                };
                if let Some(w) = warning {
                    msg.push_str(&format!(" · {w}"));
                }
                if open {
                    self.close_library();
                }
                self.notify(msg);
            }
            LibraryDone::Applied { identity, key, resolution } => {
                let title = resolution.candidate.title.clone();
                self.state.choices.insert(key, resolution.candidate.clone());
                self.state.save();
                if self.track_identity.as_deref() == Some(identity.as_str()) {
                    self.temporary = None;
                    self.show_now(resolution);
                }
                if open {
                    self.close_library();
                }
                self.notify(if local_on {
                    format!("Using “{title}” from the local library for this track")
                } else {
                    "Chosen, but the Local library provider is off: Esc → Providers".to_string()
                });
            }
            LibraryDone::Removed(entry) => {
                let local = crate::providers::local::ID;
                let before = self.state.choices.len();
                self.state.choices.retain(|_, c| !(c.provider == local && c.id == entry.id));
                if self.state.choices.len() != before {
                    self.state.save();
                }
                let showing = matches!(&self.lyrics, LyricsState::Ready { resolution, .. }
                    if resolution.candidate.provider == local && resolution.candidate.id == entry.id);
                if showing {
                    self.schedule_fetch(Duration::ZERO);
                }
                let entries = self.library.entries();
                if let Overlay::Library(d) = &mut self.overlay {
                    d.set_entries(entries);
                }
                self.notify(format!("Removed “{}” from the library", entry.track.title));
            }
        }
    }

    fn toggle<F: FnOnce(&mut Config)>(&mut self, f: F) {
        let old = self.cfg.clone();
        f(&mut self.cfg);
        self.apply_config(&old);
    }

    fn scroll_by(&mut self, rows: f64) {
        if !matches!(self.lyrics, LyricsState::Ready { .. }) {
            return;
        }
        if !self.follow.manual {
            self.follow.manual = true;
            self.follow.offset = self.frame.base;
        }
        let max = (self.frame.total - self.frame.viewport * 0.4).max(0.0);
        let min = -(self.frame.viewport * 0.4).floor();
        self.follow.offset = (self.follow.offset + rows).clamp(min, max);
        self.last_scroll = Instant::now();
    }

    fn progress_position(&self, column: u16) -> Option<f64> {
        let (_, x0, width) = self.progress_bar?;
        let len = self.track.as_ref().and_then(|t| t.length).filter(|l| *l > 0.0)?;
        let frac = ((column as f64 - x0 as f64 + 0.5) / width.max(1) as f64).clamp(0.0, 1.0);
        Some(frac * len)
    }

    fn on_press(&mut self, column: u16, row: u16) {
        match self.ui.at(column, row) {
            Some(Target::Action(action)) => {
                self.on_action(action);
                return;
            }
            Some(Target::Row(i)) if self.guide.is_some() => {
                self.guide_click(i);
                return;
            }
            Some(Target::Decrease) if self.guide.is_some() => {
                self.guide_press(KeyCode::Left);
                return;
            }
            Some(Target::Increase) if self.guide.is_some() => {
                self.guide_press(KeyCode::Right);
                return;
            }
            _ => {}
        }
        if self.ui.in_guide(column, row) {
            return;
        }
        if let Some((y, x0, width)) = self.progress_bar
            && row == y
            && column + 1 >= x0
            && column <= x0 + width
        {
            if self.controls_allowed()
                && let Some(pos) = self.progress_position(column)
            {
                self.scrub = Some(pos);
            }
            return;
        }
        if self.cfg.scroll.click_seek == crate::config::ClickSeek::Off {
            return;
        }
        let Some(hit) = self
            .frame
            .hits
            .iter()
            .find(|h| h.y == row && column >= h.x0 && column < h.x1)
            .copied()
        else {
            return;
        };
        if !self.controls_allowed() {
            return;
        }
        let offset = (self.cfg.sync.offset_ms + self.track_offset_ms()) as f64 / 1000.0;
        self.seek_to(hit.time - offset - 0.05);
    }

    fn seek_to(&mut self, position: f64) {
        let len = self.track.as_ref().and_then(|t| t.length).filter(|l| *l > 0.0);
        let target = match len {
            Some(l) => position.clamp(0.0, l),
            None => position.max(0.0),
        };
        let _ = self.player_tx.send(PlayerCommand::SetPosition(target));
        self.clock.jump(target, Instant::now());
        self.follow.manual = false;
        self.dirty = true;
    }

    fn controls_allowed(&mut self) -> bool {
        if !self.cfg.controls.enabled {
            self.notify("Playback controls are off: Esc → Controls");
            return false;
        }
        match &self.snapshot {
            None => {
                self.notify("No player to control");
                false
            }
            Some(s) if !s.caps.can_control => {
                let name = s.player.display_name();
                self.notify(format!("{name} does not accept remote control"));
                false
            }
            Some(_) => true,
        }
    }

    fn on_action(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
            Action::Help => self.set_overlay(Overlay::Help),
            Action::Settings => self.open_settings(),
            Action::Players => self.open_players(),
            Action::Syncs => self.open_syncs(),
            Action::Library => self.open_library(),
            Action::Updates => self.open_updates(),
            Action::Refetch => {
                if self.track.is_some() {
                    if let Some(key) = self.track_key() {
                        let ids: Vec<&str> = crate::providers::KNOWN_PROVIDERS.iter().map(|p| p.id).collect();
                        self.cache.forget_track(&ids, &key);
                    }
                    self.bypass_cache = true;
                    self.schedule_fetch(Duration::ZERO);
                    self.notify("Fetching lyrics again");
                }
            }
            Action::ToggleStyle => {
                self.toggle(|c| {
                    c.appearance.style = match c.appearance.style {
                        VisualStyle::Spring => VisualStyle::Bloom,
                        VisualStyle::Bloom => VisualStyle::Spring,
                    }
                });
                let name = match self.cfg.appearance.style {
                    VisualStyle::Spring => "Spring",
                    VisualStyle::Bloom => "Bloom",
                };
                self.notify(format!("Style: {name}"));
            }
            Action::ToggleRenderer => {
                if !self.pixel_supported() {
                    return;
                }
                self.toggle(|c| {
                    c.appearance.renderer = match c.appearance.renderer {
                        Renderer::Cell => Renderer::Pixel,
                        Renderer::Pixel => Renderer::Cell,
                    }
                });
                let name = if self.use_pixel() { "pixel" } else { "cell" };
                self.notify(format!("Renderer: {name}"));
            }
            Action::ToggleTranslation => {
                self.toggle(|c| c.lyrics.translation = !c.lyrics.translation);
                let available = matches!(&self.lyrics, LyricsState::Ready { prepared, .. } if prepared.lyrics.has_translation());
                let state = if self.cfg.lyrics.translation { "on" } else { "off" };
                self.notify(if available || !self.cfg.lyrics.translation {
                    format!("Translation {state}")
                } else {
                    "Translation on (none available for this sync)".into()
                });
            }
            Action::ToggleRomanization => {
                self.toggle(|c| c.lyrics.romanization = !c.lyrics.romanization);
                let available = matches!(&self.lyrics, LyricsState::Ready { prepared, .. } if prepared.lyrics.has_romanization());
                let state = if self.cfg.lyrics.romanization { "on" } else { "off" };
                self.notify(if available || !self.cfg.lyrics.romanization {
                    format!("Romanization {state}")
                } else {
                    "Romanization on (none available for this sync)".into()
                });
            }
            Action::ToggleBackgroundVocals => {
                self.toggle(|c| c.lyrics.background_vocals = !c.lyrics.background_vocals);
                let state = if self.cfg.lyrics.background_vocals { "shown" } else { "hidden" };
                self.notify(format!("Background vocals {state}"));
            }
            Action::ToggleHeader => self.toggle(|c| c.appearance.show_header = !c.appearance.show_header),
            Action::Scroll(n) => self.scroll_by(n),
            Action::Page(f) => {
                let rows = (self.frame.viewport * f).round();
                self.scroll_by(rows);
            }
            Action::ScrollTop => self.scroll_by(-1e9),
            Action::ScrollBottom => self.scroll_by(1e9),
            Action::Follow => {
                self.follow.manual = false;
            }
            Action::Offset(ms) => {
                if let Some(key) = self.track_key() {
                    let v = self.state.offset_for(&key) + ms;
                    self.state.set_offset(&key, v);
                    self.state.save();
                    self.notify(format!("Track offset {v:+} ms"));
                }
            }
            Action::OffsetReset => {
                if let Some(key) = self.track_key() {
                    self.state.set_offset(&key, 0);
                    self.state.save();
                    self.notify("Track offset reset");
                }
            }
            Action::PlayPause => {
                if self.controls_allowed() {
                    let _ = self.player_tx.send(PlayerCommand::PlayPause);
                }
            }
            Action::Seek(dir) => {
                if self.controls_allowed() {
                    let step = self.cfg.controls.seek_step_secs as f64 * dir as f64;
                    let now = Instant::now();
                    let target = self.clock.raw(now) + step;
                    let _ = self.player_tx.send(PlayerCommand::SeekBy(step));
                    let len = self.track.as_ref().and_then(|t| t.length).unwrap_or(f64::MAX);
                    self.clock.jump(target.clamp(0.0, len), now);
                }
            }
            Action::Next => {
                if self.controls_allowed() {
                    let _ = self.player_tx.send(PlayerCommand::Next);
                }
            }
            Action::Previous => {
                if self.controls_allowed() {
                    let _ = self.player_tx.send(PlayerCommand::Previous);
                }
            }
            Action::SeekToCursor => {
                if !self.follow.manual {
                    return;
                }
                let Some(item) = self.frame.cursor_item else { return };
                let Some((prepared, _)) = self.prepared() else { return };
                if prepared.lyrics.kind == SyncKind::Static || !self.controls_allowed() {
                    return;
                }
                let start = match prepared.items.get(item).map(|i| (i.kind, i.start)) {
                    Some((ItemKind::Line(_), s)) | Some((ItemKind::Interlude { .. }, s)) => s,
                    None => return,
                };
                let offset = (self.cfg.sync.offset_ms + self.track_offset_ms()) as f64 / 1000.0;
                self.seek_to(start - offset);
            }
        }
    }
}

impl App {
    pub fn maybe_onboard(&mut self, fresh: bool) {
        match self.state.onboarding {
            OnboardingStatus::Started => self.start_onboarding(),
            OnboardingStatus::Unknown if fresh => self.start_onboarding(),
            _ => {}
        }
    }

    pub fn start_onboarding(&mut self) {
        self.set_overlay(Overlay::None);
        if self.state.onboarding != OnboardingStatus::Started {
            self.state.onboarding = OnboardingStatus::Started;
            self.state.save();
        }
        self.toast = None;
        let mut guide = Onboarding::new(self.pixel_supported());
        if !self.cfg.providers.local_library.default_dir.trim().is_empty() {
            guide.stores_syncs = Some(true);
        }
        let first = guide.first();
        self.guide = Some(guide);
        self.guide_go(first);
    }

    fn guide_next(&self) -> Option<Step> {
        self.guide.as_ref().and_then(Onboarding::next)
    }

    fn guide_prev(&self) -> Option<Step> {
        self.guide.as_ref().and_then(Onboarding::prev)
    }

    fn finish_onboarding(&mut self, skipped: bool) {
        self.guide = None;
        self.state.onboarding = OnboardingStatus::Done;
        self.state.save();
        self.notify(if skipped {
            "Tour skipped · Settings → About → Welcome tour brings it back"
        } else {
            "Enjoy the music · ? shows every key"
        });
    }

    fn guide_go(&mut self, step: Step) {
        let Some(mut g) = self.guide.take() else { return };
        g.go(step);
        g.selected = g.default_row(&g.rows(self));
        self.guide = Some(g);
        self.dirty = true;
    }

    fn guide_tick(&mut self, now: Instant) {
        let synced = matches!(&self.lyrics, LyricsState::Ready { prepared, .. } if prepared.lyrics.kind != SyncKind::Static);
        let idle = matches!(self.overlay, Overlay::None);
        let Some(g) = self.guide.as_mut() else { return };
        if g.step != Step::Song || !g.auto_advance {
            return;
        }
        if !synced {
            if g.found_at.take().is_some() {
                self.dirty = true;
            }
            return;
        }
        let at = *g.found_at.get_or_insert(now);
        if idle && g.editing.is_none() && now.saturating_duration_since(at) >= onboarding::ADVANCE_AFTER {
            self.guide_go(Step::Customize);
        }
    }

    fn guide_state(&self) -> Option<(Vec<GuideRow>, usize, Step, bool)> {
        let g = self.guide.as_ref()?;
        let rows = g.rows(self);
        if rows.is_empty() {
            return None;
        }
        let sel = g.selected.min(rows.len() - 1);
        Some((rows, sel, g.step, g.editing.is_some()))
    }

    fn guide_select(&mut self, i: usize) {
        if let Some(g) = self.guide.as_mut() {
            g.selected = i;
        }
    }

    fn guide_press(&mut self, code: KeyCode) {
        self.on_guide_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn guide_change(&mut self, kind: &Kind, dir: i32, shift: bool) {
        let old = self.cfg.clone();
        if settings::step_value(&mut self.cfg, kind, dir, shift) {
            self.apply_config(&old);
        }
    }

    fn guide_toggle_provider(&mut self, id: &str) {
        let old = self.cfg.clone();
        let on = self.cfg.providers.is_enabled(id);
        self.cfg.providers.set_enabled(id, !on);
        self.apply_config(&old);
    }

    fn on_guide_key(&mut self, key: KeyEvent) -> bool {
        let Some((rows, sel, _, editing)) = self.guide_state() else { return false };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if editing {
            let Some(g) = self.guide.as_mut() else { return false };
            match key.code {
                KeyCode::Esc => g.editing = None,
                KeyCode::Enter => {
                    let value = g.editing.take().unwrap_or_default();
                    if let Some(Kind::Text { set, .. }) = rows[sel].kind() {
                        let old = self.cfg.clone();
                        set(&mut self.cfg, value);
                        self.apply_config(&old);
                    }
                }
                KeyCode::Tab if rows[sel].is_folder() => {
                    if let Some(buf) = g.editing.as_mut()
                        && let Some(done) = library_ui::complete_path(buf, true)
                    {
                        *buf = done;
                    }
                }
                KeyCode::Backspace => {
                    if let Some(buf) = g.editing.as_mut() {
                        buf.pop();
                    }
                }
                KeyCode::Char('u') if ctrl => g.editing = Some(String::new()),
                KeyCode::Char(c) if !ctrl => {
                    if let Some(buf) = g.editing.as_mut() {
                        buf.push(c);
                    }
                }
                _ => {}
            }
            return true;
        }
        let row = &rows[sel];
        let reorder = matches!(key.code, KeyCode::Char('K') | KeyCode::Char('J'))
            || (shift && matches!(key.code, KeyCode::Up | KeyCode::Down));
        match key.code {
            _ if reorder && matches!(row, GuideRow::Provider(_)) => {
                let GuideRow::Provider(id) = row else { return true };
                let delta = if matches!(key.code, KeyCode::Up | KeyCode::Char('K')) { -1 } else { 1 };
                let old = self.cfg.clone();
                let mut ids = ordered_ids(&self.cfg);
                if let Some(pos) = ids.iter().position(|p| p == id) {
                    let to = (pos as i32 + delta).clamp(0, ids.len() as i32 - 1) as usize;
                    let moved = ids.remove(pos);
                    ids.insert(to, moved);
                    self.cfg.providers.order = ids;
                    self.apply_config(&old);
                    self.guide_select(to);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.guide_select(sel.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.guide_select((sel + 1).min(rows.len() - 1)),
            KeyCode::BackTab => self.guide_select(sel.saturating_sub(1)),
            KeyCode::Home => self.guide_select(0),
            KeyCode::End => self.guide_select(rows.len() - 1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
                let dir = if matches!(key.code, KeyCode::Left | KeyCode::Char('h')) { -1 } else { 1 };
                match row {
                    GuideRow::Provider(id) => self.guide_toggle_provider(id),
                    GuideRow::Setting(it) if settings::adjustable(&it.kind) => self.guide_change(&it.kind, dir, shift),
                    _ => {}
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.guide_activate(row),
            KeyCode::Backspace | KeyCode::Delete if row.is_text() => {
                if let Some(Kind::Text { set, .. }) = row.kind() {
                    let old = self.cfg.clone();
                    set(&mut self.cfg, String::new());
                    self.apply_config(&old);
                }
            }
            KeyCode::Esc => match self.guide_prev() {
                Some(prev) => self.guide_go(prev),
                None => return false,
            },
            _ => return false,
        }
        self.dirty = true;
        true
    }

    fn guide_activate(&mut self, row: &GuideRow) {
        match row {
            GuideRow::Auto => {
                let old = self.cfg.clone();
                self.cfg.player.mode = PlayerMode::Auto;
                self.apply_config(&old);
            }
            GuideRow::Player(i) => {
                let Some(target) = self
                    .players
                    .get(*i)
                    .map(|p| p.process.clone().unwrap_or_else(|| p.short_bus().to_string()))
                else {
                    return;
                };
                let old = self.cfg.clone();
                self.cfg.player.mode = PlayerMode::Fixed;
                self.cfg.player.target = target;
                self.apply_config(&old);
            }
            GuideRow::Setting(it) => match &it.kind {
                Kind::Text { get, secret, .. } => {
                    let start = if *secret { String::new() } else { get(&self.cfg) };
                    if let Some(g) = self.guide.as_mut() {
                        g.editing = Some(start);
                    }
                }
                kind => self.guide_change(kind, 1, false),
            },
            GuideRow::Provider(id) => self.guide_toggle_provider(id),
            GuideRow::Answer(yes) => {
                let Some(mut g) = self.guide.take() else { return };
                g.stores_syncs = Some(*yes);
                if *yes {
                    let rows = g.rows(self);
                    if let Some(i) = rows.iter().position(GuideRow::is_folder) {
                        g.selected = i;
                        let current = self.cfg.providers.local_library.default_dir.clone();
                        g.editing = Some(if current.is_empty() { "~/".into() } else { current });
                    }
                    self.guide = Some(g);
                } else {
                    self.guide = Some(g);
                    if let Some(next) = self.guide_next() {
                        self.guide_go(next);
                    }
                }
            }
            GuideRow::Next => match self.guide_next() {
                Some(next) => self.guide_go(next),
                None => self.finish_onboarding(false),
            },
            GuideRow::Back => {
                if let Some(prev) = self.guide_prev() {
                    self.guide_go(prev);
                }
            }
            GuideRow::Skip => self.finish_onboarding(true),
        }
    }

    fn guide_click(&mut self, i: usize) {
        let Some((_, sel, _, editing)) = self.guide_state() else { return };
        let same = sel == i;
        if editing {
            if same {
                return;
            }
            self.guide_press(KeyCode::Enter);
        }
        self.guide_select(i);
        if same {
            self.guide_press(KeyCode::Enter);
        }
    }

    fn guide_paste(&mut self, text: &str) {
        let Some((rows, sel, _, _)) = self.guide_state() else { return };
        let text = text.trim_matches(['\n', '\r']);
        let is_text = rows[sel].is_text();
        let Some(g) = self.guide.as_mut() else { return };
        match g.editing.as_mut() {
            Some(buf) => buf.push_str(text),
            None if is_text => g.editing = Some(text.to_string()),
            None => {}
        }
    }
}

pub async fn import(mut req: ImportRequest, client: reqwest::Client, lib: Library) -> Result<LibraryDone, String> {
    let text = match req.pasted.take() {
        Some(p) => p,
        None => library::read_source(&req.source, &client).await?,
    };
    if text.len() > library::MAX_BYTES {
        return Err("the lyrics are larger than 8 MB".into());
    }
    if req.temporary {
        return temporary_import(req, &text);
    }
    let (lyrics, detected) = library::parse(&text, &req.source, req.format, req.track.duration)?;
    let warning = library::title_warning(&text, detected, &req.track);
    let key = req.track.key();
    let now = library::now();
    let entry = Entry {
        id: Library::id_for(&key),
        key: key.clone(),
        track: req.track,
        source: req.source,
        format: req.format,
        detected: Some(detected),
        kind: Some(lyrics.kind),
        watch: req.watch,
        file: String::new(),
        added_at: now,
        updated_at: now,
    };
    let entry = tokio::task::spawn_blocking(move || lib.put(entry, &text))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("could not save the lyrics: {e}"))?;
    Ok(LibraryDone::Imported {
        identity: req.identity,
        key,
        resolution: Resolution {
            candidate: crate::providers::local::entry_candidate(&entry, Some(MatchTier::Exact)),
            lyrics,
            manual: false,
        },
        temporary: false,
        warning,
    })
}

pub fn temporary_import(req: ImportRequest, text: &str) -> Result<LibraryDone, String> {
    if text.len() > library::MAX_BYTES {
        return Err("the lyrics are larger than 8 MB".into());
    }
    let (lyrics, detected) = library::parse(text, &req.source, req.format, req.track.duration)?;
    let warning = library::title_warning(text, detected, &req.track);
    let candidate = Candidate {
        provider: crate::providers::local::ID.into(),
        id: TEMPORARY_ID.into(),
        title: req.track.title.clone(),
        artists: req.track.artists.clone(),
        album: req.track.album.clone(),
        duration: req.track.duration,
        tier: Some(MatchTier::Exact),
        kind_hint: Some(lyrics.kind),
        note: Some(format!("temporary {} {}", req.source.label(), req.source.name())),
    };
    Ok(LibraryDone::Imported {
        identity: req.identity,
        key: req.track.key(),
        resolution: Resolution {
            candidate,
            lyrics,
            manual: true,
        },
        temporary: true,
        warning,
    })
}

mod erased {
    pub trait Json {
        fn json(&self) -> String;
    }

    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> String {
            serde_json::to_string(self).unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let (player_tx, _player_rx) = tokio::sync::mpsc::unbounded_channel();
        let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(Config::default(), Config::stamp(), TermCaps::default(), player_tx, msg_tx);
        app.guide = Some(Onboarding::new(false));
        app.guide_go(Step::Player);
        app
    }

    fn snapshot() -> PlayerEvent {
        PlayerEvent::Active(Some(Snapshot {
            player: PlayerInfo {
                bus: "org.mpris.MediaPlayer2.test".into(),
                identity: "Test Player".into(),
                process: Some("testplayer".into()),
                pid: None,
                desktop_entry: None,
                status: Status::Playing,
                now_playing: None,
            },
            track: Some(TrackMeta {
                title: "Tour Song".into(),
                artists: vec!["Nobody".into()],
                length: Some(30.0),
                ..Default::default()
            }),
            status: Status::Playing,
            rate: 1.0,
            position: 0.0,
            sampled_at: Instant::now(),
            caps: Default::default(),
        }))
    }

    fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            crate::ui::draw::draw(f, app, Instant::now(), 0.0);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn step(app: &App) -> Step {
        app.guide.as_ref().unwrap().step
    }

    #[tokio::test]
    async fn settings_reopen_where_they_were_left() {
        let mut app = app();
        app.guide = None;
        let spot = |app: &App| match &app.overlay {
            Overlay::Settings(s) => Some((s.section, s.item)),
            _ => None,
        };
        app.on_action(Action::Settings);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(spot(&app), Some((1, 2)));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.overlay, Overlay::None));
        app.on_action(Action::Settings);
        assert_eq!(spot(&app), Some((1, 2)));
        app.set_overlay(Overlay::None);
        if let Some((_, at)) = app.settings_left.as_mut() {
            *at -= Duration::from_secs(31);
        }
        app.on_action(Action::Settings);
        assert_eq!(spot(&app), Some((0, 0)));
    }

    #[tokio::test]
    async fn text_only_tour_starts_with_the_terminal() {
        let mut app = app();
        app.guide = Some(Onboarding::new(false));
        app.guide_go(Step::Terminal);
        let shown = screen(&mut app, 160, 40);
        assert!(shown.iter().any(|l| l.contains("Your terminal")));
        assert!(shown.iter().any(|l| l.contains("Continue with the text look")));
        assert!(shown.iter().any(|l| l.contains(" 1 / 7 ")));
        press(&mut app, KeyCode::Enter);
        assert_eq!(step(&app), Step::Player);
        press(&mut app, KeyCode::Esc);
        assert_eq!(step(&app), Step::Terminal);
    }

    #[tokio::test]
    async fn tour_walks_from_the_player_to_the_looks() {
        let mut app = app();
        app.on_player(snapshot());
        let wide = screen(&mut app, 160, 40);
        assert!(wide[0].starts_with("╭ Welcome to Lyrminal"));
        assert!(wide.iter().any(|l| l.contains("✓ Following Test Player")));
        let narrow = screen(&mut app, 80, 40);
        assert!(narrow[0].starts_with("╭ Welcome to Lyrminal"));
        assert!(narrow[17].starts_with("╰"));

        press(&mut app, KeyCode::Enter);
        assert_eq!(step(&app), Step::Providers);
        press(&mut app, KeyCode::Esc);
        assert_eq!(step(&app), Step::Player);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert_eq!(step(&app), Step::Library);
        let shown = screen(&mut app, 160, 40);
        assert!(shown.iter().any(|l| l.contains("Your own syncs")));
        assert!(shown.iter().any(|l| l.contains("Yes, I keep sync files")));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(step(&app), Step::Song);

        let (lyrics, _) = library::parse("[00:01.00]first line\n[00:05.00]second line\n", &Source::Clipboard, None, Some(30.0)).unwrap();
        app.show_now(Resolution {
            candidate: Candidate {
                provider: crate::providers::lrclib::ID.into(),
                id: "1".into(),
                title: "Tour Song".into(),
                artists: vec!["Nobody".into()],
                album: None,
                duration: Some(30.0),
                tier: None,
                kind_hint: None,
                note: None,
            },
            lyrics,
            manual: false,
        });
        let now = Instant::now();
        app.tick(now);
        assert_eq!(step(&app), Step::Song);
        assert!(screen(&mut app, 160, 40).iter().any(|l| l.contains("✓ Line sync from LRCLIB")));
        app.tick(now + onboarding::ADVANCE_AFTER);
        assert_eq!(step(&app), Step::Customize);
        assert_eq!(app.guide.as_ref().unwrap().selected, 0);
        let looks = screen(&mut app, 160, 40);
        assert!(looks.iter().any(|l| l.contains("Tip: the Spicy Lyrics preset")));
        assert!(looks.iter().any(|l| l.contains("› Preset") && l.contains("‹ Lyrminal ›")));
        assert!(looks.iter().any(|l| l[l.char_indices().nth(60).unwrap().0..].contains("first line")));

        press(&mut app, KeyCode::Char('?'));
        assert!(matches!(app.overlay, Overlay::Help));
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.guide.as_ref().unwrap().selected, 1);

        press(&mut app, KeyCode::Esc);
        assert_eq!(step(&app), Step::Song);
        app.tick(now + onboarding::ADVANCE_AFTER * 2);
        app.tick(now + onboarding::ADVANCE_AFTER * 4);
        assert_eq!(step(&app), Step::Song);
    }

    #[tokio::test]
    async fn tour_offers_a_library_folder_to_people_who_keep_syncs() {
        let mut app = app();
        app.guide_go(Step::Library);
        let rows = app.guide.as_ref().unwrap().rows(&app);
        assert!(!rows.iter().any(GuideRow::is_folder));
        assert_eq!(app.guide.as_ref().unwrap().selected, 0);
        press(&mut app, KeyCode::Enter);
        let g = app.guide.as_ref().unwrap();
        assert_eq!(g.stores_syncs, Some(true));
        assert_eq!(g.editing.as_deref(), Some("~/"));
        assert!(g.rows(&app)[g.selected].is_folder());
        let shown = screen(&mut app, 160, 40);
        assert!(shown.iter().any(|l| l.contains("Library location")));
        assert!(shown.iter().any(|l| l.contains("Tab complete")));

        let dir = std::env::temp_dir().join(format!("lyrminal-tour-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("syncs-folder")).unwrap();
        app.guide.as_mut().unwrap().editing = Some(format!("{}/syncs-f", dir.display()));
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.guide.as_ref().unwrap().editing.clone(), Some(format!("{}/syncs-folder/", dir.display())));
        press(&mut app, KeyCode::Esc);
        assert!(app.guide.as_ref().unwrap().editing.is_none());
        assert_eq!(step(&app), Step::Library);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn offers_a_found_update_and_restarts_into_it() {
        let mut app = app();
        app.guide = None;
        let release = Release {
            version: "9.9.9".into(),
            tag: "v9.9.9".into(),
            url: "https://github.com/BajekekButLost/lyrminal/releases/tag/v9.9.9".into(),
            summary: "Big one.".into(),
            asset: Some(update::Asset {
                name: "lyrminal-v9.9.9-x86_64-linux".into(),
                url: "https://example.com/lyrminal".into(),
                size: 10,
            }),
        };
        app.on_msg(AppMsg::UpdateChecked {
            manual: false,
            result: Ok(Some(release.clone())),
        });
        assert!(matches!(&app.update, UpdateStatus::Available(r) if r.version == "9.9.9"));
        assert!(screen(&mut app, 160, 40)[39].contains("update v9.9.9"));

        press(&mut app, KeyCode::Char('U'));
        let Overlay::Settings(s) = &app.overlay else { panic!("settings should be open") };
        assert_eq!(s.sections(&app.cfg)[s.section].title, settings::UPDATES_SECTION);
        let shown = screen(&mut app, 160, 40);
        assert!(shown.iter().any(|l| l.contains("v9.9.9 available · install ↵")));
        assert!(shown.iter().any(|l| l.contains("Big one.")));

        app.update = UpdateStatus::Downloading { release, done: 0, total: 10 };
        app.on_msg(AppMsg::UpdateProgress { done: 5, total: 10 });
        assert!(screen(&mut app, 160, 40).iter().any(|l| l.contains("downloading 50%")));
        let path = std::path::PathBuf::from("/tmp/lyrminal-new");
        app.on_msg(AppMsg::UpdateInstalled(Ok(path.clone())));
        assert!(matches!(app.update, UpdateStatus::Installed { .. }));
        press(&mut app, KeyCode::Enter);
        assert!(app.quit);
        assert_eq!(app.restart_into, Some(path));
    }

    #[tokio::test]
    async fn library_changes_during_a_lookup_start_it_again() {
        let mut app = app();
        app.guide = None;
        app.on_player(snapshot());
        app.lyrics = LyricsState::Loading;
        let before = app.generation;
        app.library_changed(&crate::ipc::LibraryChange {
            changed: vec!["abc".into()],
            ..Default::default()
        });
        assert!(app.generation > before);
        assert!(matches!(app.lyrics, LyricsState::Waiting));
        app.on_msg(AppMsg::Resolved {
            generation: before,
            outcome: Outcome {
                result: None,
                attempts: Vec::new(),
                rate_limits: Vec::new(),
            },
        });
        assert!(matches!(app.lyrics, LyricsState::Waiting));
        let quiet = app.generation;
        app.lyrics = LyricsState::Loading;
        app.library_changed(&crate::ipc::LibraryChange::default());
        assert_eq!(app.generation, quiet);
    }

    #[tokio::test]
    async fn quiet_checks_only_speak_up_for_news() {
        let mut app = app();
        app.toast = None;
        app.on_msg(AppMsg::UpdateChecked { manual: false, result: Ok(None) });
        assert!(matches!(app.update, UpdateStatus::UpToDate));
        assert!(app.toast.is_none());
        app.on_msg(AppMsg::UpdateChecked { manual: false, result: Err("offline".into()) });
        assert!(app.toast.is_none());
        app.on_msg(AppMsg::UpdateChecked { manual: true, result: Ok(None) });
        assert!(app.toast.as_ref().is_some_and(|(m, _)| m.contains("is up to date")));
        app.tick(Instant::now());
        assert!(matches!(app.update, UpdateStatus::UpToDate));
    }

    #[tokio::test]
    async fn pasting_on_the_key_row_starts_editing() {
        let mut app = app();
        app.guide_go(Step::Providers);
        let rows = app.guide.as_ref().unwrap().rows(&app);
        let key_row = rows.iter().position(|r| r.is_text()).unwrap();
        app.guide_select(key_row);
        app.on_event(Event::Paste("sl_sk_secret\n".into()));
        assert_eq!(app.guide.as_ref().unwrap().editing.as_deref(), Some("sl_sk_secret"));
        let shown = screen(&mut app, 160, 40);
        assert!(shown.iter().any(|l| l.contains("••••••••••••▏")));
        assert!(!shown.iter().any(|l| l.contains("sl_sk_secret")));
        press(&mut app, KeyCode::Esc);
        assert!(app.guide.as_ref().unwrap().editing.is_none());
        assert_eq!(step(&app), Step::Providers);
    }
}
