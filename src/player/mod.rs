pub mod clock;
#[cfg(target_os = "linux")]
mod mpris;

use std::time::Instant;

use tokio::sync::mpsc;

use crate::config::{PlayerConfig, PlayerMode};
use crate::providers::TrackQuery;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Playing,
    Paused,
    Stopped,
}

impl Status {
    pub fn parse(s: &str) -> Self {
        match s {
            "Playing" => Status::Playing,
            "Paused" => Status::Paused,
            _ => Status::Stopped,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Status::Playing => "▶",
            Status::Paused => "⏸",
            Status::Stopped => "■",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Track,
    Episode,
    Ad,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfo {
    pub bus: String,
    pub identity: String,
    pub process: Option<String>,
    pub pid: Option<u32>,
    pub desktop_entry: Option<String>,
    pub status: Status,
    pub now_playing: Option<String>,
}

impl PlayerInfo {
    pub fn short_bus(&self) -> &str {
        self.bus.strip_prefix(BUS_PREFIX).unwrap_or(&self.bus)
    }

    pub fn display_name(&self) -> String {
        if self.identity.is_empty() {
            self.short_bus().to_string()
        } else {
            self.identity.clone()
        }
    }

    pub fn matches(&self, target: &str) -> bool {
        let t = target.trim().to_lowercase();
        if t.is_empty() {
            return false;
        }
        let short = self.short_bus().to_lowercase();
        self.process.as_deref().is_some_and(|p| p.to_lowercase() == t)
            || short == t
            || short.split('.').next().is_some_and(|s| s == t)
            || self.identity.to_lowercase() == t
            || self.desktop_entry.as_deref().is_some_and(|d| d.to_lowercase() == t)
    }
}

pub const BUS_PREFIX: &str = "org.mpris.MediaPlayer2.";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackMeta {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artists: Vec<String>,
    pub length: Option<f64>,
    pub track_id: Option<String>,
    pub url: Option<String>,
    pub art_url: Option<String>,
    pub spotify_id: Option<String>,
    pub isrc: Option<String>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub kind: Option<MediaKind>,
}

impl TrackMeta {
    pub fn identity_key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.spotify_id.as_deref().unwrap_or(""),
            self.title,
            self.artists.join(","),
            self.album.as_deref().unwrap_or(""),
            self.track_id.as_deref().unwrap_or("")
        )
    }

    pub fn query(&self) -> TrackQuery {
        let artists = if self.artists.is_empty() {
            self.album_artists.clone()
        } else {
            self.artists.clone()
        };
        TrackQuery {
            title: self.title.clone(),
            artists,
            album: self.album.clone().filter(|a| !a.is_empty()),
            duration: self.length.filter(|l| *l > 0.0),
            spotify_id: self.spotify_id.clone(),
            isrc: self.isrc.clone(),
        }
    }

    pub fn media_kind(&self) -> MediaKind {
        self.kind.unwrap_or(MediaKind::Track)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Capabilities {
    pub can_control: bool,
    pub can_seek: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play: bool,
    pub can_pause: bool,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub player: PlayerInfo,
    pub track: Option<TrackMeta>,
    pub status: Status,
    pub rate: f64,
    pub position: f64,
    pub sampled_at: Instant,
    pub caps: Capabilities,
}

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    Players(Vec<PlayerInfo>),
    Active(Option<Snapshot>),
    Position { position: f64, at: Instant, seeked: bool },
    Error(String),
}

#[derive(Debug, Clone)]
pub enum PlayerCommand {
    Reconfigure(PlayerConfig),
    PlayPause,
    Next,
    Previous,
    SeekBy(f64),
    SetPosition(f64),
}

pub fn choose<'a>(players: &'a [PlayerInfo], cfg: &PlayerConfig, current: Option<&str>) -> Option<&'a PlayerInfo> {
    match cfg.mode {
        PlayerMode::Fixed => players.iter().find(|p| p.matches(&cfg.target)),
        PlayerMode::Auto => {
            let pref_rank = |p: &PlayerInfo| {
                cfg.preferred
                    .iter()
                    .position(|t| p.matches(t))
                    .unwrap_or(usize::MAX)
            };
            let playing: Vec<&PlayerInfo> = players.iter().filter(|p| p.status == Status::Playing).collect();
            if !playing.is_empty() {
                if let Some(cur) = current
                    && let Some(p) = playing.iter().find(|p| p.bus == cur)
                {
                    let best = playing.iter().min_by_key(|p| pref_rank(p)).copied();
                    if best.is_none_or(|b| pref_rank(b) >= pref_rank(p)) {
                        return Some(p);
                    }
                }
                return playing.into_iter().min_by_key(|p| pref_rank(p));
            }
            if let Some(cur) = current
                && let Some(p) = players.iter().find(|p| p.bus == cur)
            {
                return Some(p);
            }
            players.iter().min_by_key(|p| (pref_rank(p), p.status != Status::Paused))
        }
    }
}

pub fn spawn(cfg: PlayerConfig) -> (mpsc::UnboundedSender<PlayerCommand>, mpsc::UnboundedReceiver<PlayerEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    #[cfg(target_os = "linux")]
    tokio::spawn(mpris::run(cfg, cmd_rx, ev_tx));
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cfg, cmd_rx);
        let _ = ev_tx.send(PlayerEvent::Error(
            "No media backend for this platform yet".into(),
        ));
    }
    (cmd_tx, ev_rx)
}

pub fn spotify_id_from(url: Option<&str>, track_id: Option<&str>) -> (Option<String>, Option<MediaKind>) {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(u) = url {
        candidates.push(u.to_string());
    }
    if let Some(t) = track_id {
        candidates.push(t.to_string());
        for seg in t.split('/') {
            if let Some(hex) = seg.strip_prefix('t')
                && hex.len() % 2 == 0
                && hex.len() >= 2
                && hex.chars().all(|c| c.is_ascii_hexdigit())
            {
                let bytes: Option<Vec<u8>> = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                    .collect();
                if let Some(decoded) = bytes.and_then(|b| String::from_utf8(b).ok()) {
                    candidates.push(decoded);
                }
            }
        }
    }
    for c in &candidates {
        let lower = c.to_lowercase();
        if lower.contains("spotify:ad:") || lower.contains("/com/spotify/ad/") {
            return (None, Some(MediaKind::Ad));
        }
        if lower.contains("spotify:episode:") || lower.contains("/episode/") {
            return (None, Some(MediaKind::Episode));
        }
    }
    for c in &candidates {
        for marker in ["spotify:track:", "open.spotify.com/track/", "/com/spotify/track/"] {
            if let Some(i) = c.find(marker) {
                let id: String = c[i + marker.len()..]
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric())
                    .collect();
                if id.len() == 22 {
                    return (Some(id), Some(MediaKind::Track));
                }
            }
        }
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_spotify_ids() {
        let (id, _) = spotify_id_from(Some("spotify:track:16yVaWo0Gmvs2wlnmJbcH7"), None);
        assert_eq!(id.as_deref(), Some("16yVaWo0Gmvs2wlnmJbcH7"));
        let (id, _) = spotify_id_from(
            None,
            Some("/spotifast/Track/t73706f746966793a747261636b3a3136795661576f30476d767332776c6e6d4a62634837"),
        );
        assert_eq!(id.as_deref(), Some("16yVaWo0Gmvs2wlnmJbcH7"));
        let (id, _) = spotify_id_from(Some("https://open.spotify.com/track/16yVaWo0Gmvs2wlnmJbcH7?si=x"), None);
        assert_eq!(id.as_deref(), Some("16yVaWo0Gmvs2wlnmJbcH7"));
        let (_, kind) = spotify_id_from(None, Some("/com/spotify/ad/123"));
        assert_eq!(kind, Some(MediaKind::Ad));
    }

    #[tokio::test]
    #[ignore]
    async fn live_mpris() {
        let (_tx, mut rx) = spawn(PlayerConfig::default());
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            match ev {
                PlayerEvent::Players(p) => {
                    for i in p {
                        println!("player {} process={:?} identity={} status={:?}", i.bus, i.process, i.identity, i.status);
                    }
                }
                PlayerEvent::Active(Some(s)) => {
                    let t = s.track.as_ref();
                    println!(
                        "active {} status={:?} pos={:.2} rate={} spotify_id={:?} len={:?} kind={:?} caps={:?}",
                        s.player.short_bus(),
                        s.status,
                        s.position,
                        s.rate,
                        t.and_then(|t| t.spotify_id.clone()),
                        t.and_then(|t| t.length),
                        t.map(|t| t.media_kind()),
                        s.caps
                    );
                }
                PlayerEvent::Active(None) => println!("active none"),
                PlayerEvent::Position { position, seeked, .. } => println!("position {position:.3} seeked={seeked}"),
                PlayerEvent::Error(e) => println!("error {e}"),
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn live_clock_accuracy() {
        let (_tx, mut rx) = spawn(PlayerConfig::default());
        let mut clock = clock::PlaybackClock::default();
        let mut last_value: Option<f64> = None;
        let mut started = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
        while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            match ev {
                PlayerEvent::Active(Some(s)) => {
                    if !started {
                        clock.reset(s.position, s.sampled_at, s.status == Status::Playing, s.rate);
                        started = true;
                    }
                }
                PlayerEvent::Position { position, at, seeked } => {
                    let edge = last_value.is_some_and(|v| (v - position).abs() > 1e-6);
                    clock.sample(position, at, seeked);
                    if edge {
                        println!("edge: reported {position:.3}  clock {:.3}  error {:+.0} ms", clock.raw(at), (clock.raw(at) - position) * 1000.0);
                    }
                    last_value = Some(position);
                }
                _ => {}
            }
        }
    }

    fn info(bus: &str, process: &str, status: Status) -> PlayerInfo {
        PlayerInfo {
            bus: format!("{BUS_PREFIX}{bus}"),
            identity: process.into(),
            process: Some(process.into()),
            pid: None,
            desktop_entry: None,
            status,
            now_playing: None,
        }
    }

    #[test]
    fn auto_prefers_playing_then_preferred() {
        let cfg = PlayerConfig::default();
        let players = vec![
            info("chromium.instance1", "chromium", Status::Playing),
            info("spotifast", "spotifast", Status::Paused),
        ];
        assert_eq!(choose(&players, &cfg, None).unwrap().short_bus(), "chromium.instance1");
        let players = vec![
            info("chromium.instance1", "chromium", Status::Paused),
            info("spotifast", "spotifast", Status::Paused),
        ];
        assert_eq!(choose(&players, &cfg, None).unwrap().short_bus(), "spotifast");
        let mut fixed = cfg.clone();
        fixed.mode = PlayerMode::Fixed;
        fixed.target = "chromium".into();
        assert_eq!(choose(&players, &fixed, None).unwrap().short_bus(), "chromium.instance1");
    }
}
