use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseButton, MouseEventKind};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{Config, PlayerMode, Renderer, VisualStyle};
use crate::lyrics::process::{self, ItemKind, PrepareOptions, Prepared};
use crate::lyrics::{Lyrics, SyncKind};
use crate::player::clock::PlaybackClock;
use crate::player::{MediaKind, PlayerCommand, PlayerEvent, PlayerInfo, Snapshot, Status, TrackMeta};
use crate::providers::cache::Cache;
use crate::providers::resolve::{self, Attempt, Outcome, Resolution};
use crate::providers::{Candidate, Registry, TrackQuery, ordered_ids};
use crate::render::{FrameInfo, Follow};
use crate::render::cell::CellRenderer;
use crate::render::pixel::{Fonts, PixelRenderer};
use crate::state::State;
use crate::term::TermCaps;
use crate::theme::{Artwork, Theme};
use crate::ui::keys::{self, Action};
use crate::ui::settings::{self, SettingsAction, SettingsOutcome, SettingsState};

const FETCH_DEBOUNCE: Duration = Duration::from_millis(350);
const TOAST_FOR: Duration = Duration::from_secs(3);
const MOUSE_SCROLL_ROWS: f64 = 4.0;

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
}

pub enum Overlay {
    None,
    Help,
    Settings(SettingsState),
    Players { selected: usize },
    Syncs(SyncPicker),
}

pub enum AppMsg {
    Resolved { generation: u64, outcome: Outcome },
    Artwork { url: String, result: Result<Artwork, String> },
    Search { generation: u64, provider: String, result: Result<Vec<Candidate>, String> },
    Fetched { generation: u64, candidate: Candidate, result: Result<Lyrics, String> },
    CacheCleared(Result<(), String>),
    Fonts(Box<Fonts>),
}

pub struct App {
    pub cfg: Config,
    pub state: State,
    pub caps: TermCaps,
    pub client: reqwest::Client,
    pub registry: Registry,
    pub cache: Cache,
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
    pub toast: Option<(String, Instant)>,
    pub quit: bool,
    start: Instant,
    last_frame: Instant,
    next_prepared_id: usize,
    pub frame: FrameInfo,
    pub dirty: bool,
    pub progress_bar: Option<(u16, u16, u16)>,
    pub scrub: Option<f64>,
}

impl App {
    pub fn new(
        cfg: Config,
        caps: TermCaps,
        player_tx: UnboundedSender<PlayerCommand>,
        msg_tx: UnboundedSender<AppMsg>,
    ) -> Self {
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
            toast: None,
            quit: false,
            start: now,
            last_frame: now,
            next_prepared_id: 1,
            frame: FrameInfo::default(),
            dirty: true,
            progress_bar: None,
            scrub: None,
        };
        if app.cfg.providers.spicy_lyrics.effective_key().is_none() && app.cfg.providers.is_enabled(crate::providers::spicy::ID) {
            app.notify("Spicy Lyrics needs an API key: press o → Providers");
        }
        if app.cfg.appearance.renderer == Renderer::Pixel && !app.pixel_supported() {
            app.notify("Pixel renderer needs kitty graphics; using the cell renderer");
        }
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
    }

    pub fn needs_animation(&self) -> bool {
        self.dirty
            || self.frame.animating
            || self.cell.is_animating()
            || (self.clock.is_playing() && matches!(self.lyrics, LyricsState::Ready { .. }))
            || matches!(self.lyrics, LyricsState::Loading | LyricsState::Waiting)
            || matches!(&self.overlay, Overlay::Syncs(p) if !p.pending.is_empty() || p.applying.is_some())
    }

    fn start_fetch(&mut self) {
        let Some(track) = self.track.clone() else { return };
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
        self.follow = Follow::default();
        self.cell.reset();
        self.fetch_due = None;
        self.generation += 1;
        if let Overlay::Syncs(_) = self.overlay {
            self.overlay = Overlay::None;
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
            AppMsg::Resolved { generation, outcome } => {
                if generation != self.generation {
                    return;
                }
                match outcome.result {
                    Some(res) => self.install(res),
                    None => self.lyrics = LyricsState::NotFound(outcome.attempts),
                }
            }
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
                        self.overlay = Overlay::None;
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
        }
    }

    pub fn on_event(&mut self, ev: Event) {
        self.dirty = true;
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Paste(text) => {
                if let Overlay::Settings(s) = &mut self.overlay {
                    settings::paste(s, &text);
                }
            }
            Event::Mouse(m) if self.cfg.appearance.mouse && matches!(self.overlay, Overlay::None) => match m.kind {
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
                self.overlay = Overlay::None;
                return;
            }
            Overlay::Settings(state) => {
                let old = self.cfg.clone();
                let outcome = settings::handle_key(state, &mut self.cfg, key);
                match outcome {
                    SettingsOutcome::None => {}
                    SettingsOutcome::Close => self.overlay = Overlay::None,
                    SettingsOutcome::Changed => self.apply_config(&old),
                    SettingsOutcome::Action(a) => self.settings_action(a),
                }
                return;
            }
            Overlay::Players { selected } => {
                use crossterm::event::KeyCode;
                let n = self.players.len() + 1;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('p') => self.overlay = Overlay::None,
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
                        self.overlay = Overlay::None;
                        self.apply_config(&old);
                    }
                    _ => {}
                }
                return;
            }
            Overlay::Syncs(picker) => {
                use crossterm::event::KeyCode;
                let order = ordered_ids(&self.cfg);
                let rows = picker.rows(&order);
                let selectable = |r: &SyncRow| !matches!(r, SyncRow::Header { .. });
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => self.overlay = Overlay::None,
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
                    KeyCode::Char('r') => self.open_syncs(),
                    KeyCode::Enter => match rows.get(picker.selected) {
                        Some(SyncRow::Auto) => {
                            if let Some(key) = self.track_key() {
                                self.state.choices.remove(&key);
                                self.state.save();
                            }
                            self.overlay = Overlay::None;
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
                                let result = resolve::fetch_candidate(&registry, &cache, &c, false)
                                    .await
                                    .map_err(|e| e.to_string());
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
        if let Some(action) = keys::map(key) {
            self.on_action(action);
        }
    }

    fn settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::PickPlayer => self.open_players(),
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
                let key = self.cfg.providers.spicy_lyrics.api_key.clone();
                self.cfg = Config::default();
                self.cfg.providers.spicy_lyrics.api_key = key;
                self.apply_config(&old);
                self.notify("Settings reset to defaults");
            }
        }
    }

    pub fn apply_config(&mut self, old: &Config) {
        let json = |v: &dyn erased::Json| v.json();
        if let Err(e) = self.cfg.save() {
            self.notify(format!("Could not save settings: {e}"));
        }
        let providers_changed = json(&self.cfg.providers) != json(&old.providers);
        if providers_changed {
            self.registry = Registry::from_config(&self.cfg, &self.client);
            if matches!(self.lyrics, LyricsState::NotFound(_) | LyricsState::Ready { .. }) && self.track.is_some() {
                let manual = matches!(&self.lyrics, LyricsState::Ready { resolution, .. } if resolution.manual);
                let order_changed = ordered_ids(&self.cfg) != ordered_ids(old) || self.cfg.providers.disabled != old.providers.disabled;
                let key_changed = self.cfg.providers.spicy_lyrics.api_key != old.providers.spicy_lyrics.api_key;
                if !manual && (order_changed || key_changed || matches!(self.lyrics, LyricsState::NotFound(_))) {
                    self.schedule_fetch(Duration::from_millis(600));
                }
            }
        }
        if json(&self.cfg.player) != json(&old.player) {
            let _ = self.player_tx.send(PlayerCommand::Reconfigure(self.cfg.player.clone()));
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
            if self.cfg.appearance.renderer == Renderer::Pixel
                && old.appearance.renderer != Renderer::Pixel
                && !self.pixel_supported()
            {
                self.notify("This terminal has no kitty graphics support, so the cell renderer stays on");
            }
            self.load_artwork();
            self.retheme();
            self.ensure_fonts();
        }
        let prep_changed = self.cfg.appearance.style != old.appearance.style
            || self.cfg.appearance.fade_previous != old.appearance.fade_previous
            || self.cfg.lyrics.interludes != old.lyrics.interludes
            || self.cfg.lyrics.interlude_threshold_secs != old.lyrics.interlude_threshold_secs
            || self.cfg.lyrics.join_split_words != old.lyrics.join_split_words;
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
        self.overlay = Overlay::Players { selected };
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
        self.overlay = Overlay::Syncs(SyncPicker {
            generation,
            pending,
            results: Vec::new(),
            selected: 0,
            applying: None,
            error: None,
        });
        for p in self.registry.providers.clone() {
            let q = q.clone();
            let tx = self.msg_tx.clone();
            tokio::spawn(async move {
                let result = match p.ready() {
                    Ok(()) => p.search(&q).await.map_err(|e| e.to_string()),
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
            self.notify("Playback controls are off: o → Controls");
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
            Action::Help => self.overlay = Overlay::Help,
            Action::Settings => self.overlay = Overlay::Settings(SettingsState::new()),
            Action::Players => self.open_players(),
            Action::Syncs => self.open_syncs(),
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
                if !self.pixel_supported() && self.cfg.appearance.renderer == Renderer::Cell {
                    self.notify("This terminal has no kitty graphics support");
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
