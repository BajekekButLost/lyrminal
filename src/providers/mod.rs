pub mod cache;
pub mod local;
pub mod lrclib;
pub mod lrcred;
pub mod resolve;
pub mod spicy;
pub mod unison;

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::{Config, MatchTier};
use crate::lyrics::{Lyrics, SyncKind};

pub const USER_AGENT: &str = concat!(
    "Lyrminal v",
    env!("CARGO_PKG_VERSION"),
    " (https://bajekek.com; me@bajekek.com)"
);

const MAX_WAIT: Duration = Duration::from_secs(8);

pub struct RateGate {
    min_interval: Duration,
    state: Mutex<GateState>,
}

struct GateState {
    last: Option<Instant>,
    blocked_until: Option<Instant>,
}

fn jitter_fraction() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1000) as f64 / 1000.0
}

impl RateGate {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            state: Mutex::new(GateState {
                last: None,
                blocked_until: None,
            }),
        }
    }

    pub async fn acquire(&self) -> ProviderResult<()> {
        let mut st = self.state.lock().await;
        let now = Instant::now();
        if let Some(until) = st.blocked_until {
            if until > now {
                let wait = until - now;
                if wait > MAX_WAIT {
                    return Err(ProviderError::RateLimited(Some(wait.as_secs().max(1))));
                }
                tokio::time::sleep(wait).await;
            }
            st.blocked_until = None;
        }
        if let Some(last) = st.last {
            let next = last + self.min_interval;
            let now = Instant::now();
            if next > now {
                tokio::time::sleep(next - now).await;
            }
        }
        st.last = Some(Instant::now());
        Ok(())
    }

    pub async fn penalize(&self, retry_after_secs: Option<u64>) {
        let base = retry_after_secs.unwrap_or(30).max(1) as f64;
        let wait = base * (1.0 + 0.3 * jitter_fraction());
        let mut st = self.state.lock().await;
        st.blocked_until = Some(Instant::now() + Duration::from_secs_f64(wait));
    }

    pub async fn observe<T>(&self, result: ProviderResult<T>) -> ProviderResult<T> {
        match &result {
            Err(ProviderError::RateLimited(retry)) => self.penalize(*retry).await,
            Err(ProviderError::Unavailable(_, retry)) => self.penalize(Some(retry.unwrap_or(5))).await,
            _ => {}
        }
        result
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackQuery {
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub spotify_id: Option<String>,
    pub isrc: Option<String>,
}

impl TrackQuery {
    pub fn artist_line(&self) -> String {
        self.artists.join(", ")
    }

    pub fn key(&self) -> String {
        if let Some(id) = &self.spotify_id {
            return format!("spotify:{id}");
        }
        if let Some(isrc) = &self.isrc {
            return format!("isrc:{isrc}");
        }
        format!(
            "meta:{}|{}|{}|{}",
            normalize(&self.artist_line()),
            normalize(&self.title),
            normalize(self.album.as_deref().unwrap_or("")),
            self.duration.map(|d| d.round() as i64).unwrap_or(0)
        )
    }

    pub fn is_searchable(&self) -> bool {
        !self.title.trim().is_empty()
    }
}

pub fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn clean_title(title: &str) -> String {
    let mut t = title.to_string();
    for sep in [" - ", " – ", " — "] {
        if let Some(i) = t.find(sep) {
            let tail = t[i + sep.len()..].to_lowercase();
            if ["remaster", "live", "version", "edit", "mix", "mono", "stereo", "from", "feat"]
                .iter()
                .any(|k| tail.contains(k))
            {
                t.truncate(i);
            }
        }
    }
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let Some(i) = t.find(open) {
            let Some(j) = t[i..].find(close) else { break };
            let inner = t[i + 1..i + j].to_lowercase();
            if ["feat", "ft.", "with ", "remaster", "explicit", "clean"]
                .iter()
                .any(|k| inner.contains(k))
            {
                t.replace_range(i..i + j + 1, "");
            } else {
                break;
            }
        }
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub provider: String,
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub tier: Option<MatchTier>,
    pub kind_hint: Option<SyncKind>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ProviderError {
    NotConfigured(String),
    NotFound,
    Local(String),
    RateLimited(Option<u64>),
    Unavailable(String, Option<u64>),
    Http(u16, String),
    Network(String),
    Parse(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::NotConfigured(m) => write!(f, "not configured: {m}"),
            ProviderError::Local(m) => write!(f, "{m}"),
            ProviderError::NotFound => write!(f, "no lyrics found"),
            ProviderError::RateLimited(Some(s)) => write!(f, "rate limited, retry in {s}s"),
            ProviderError::RateLimited(None) => write!(f, "rate limited"),
            ProviderError::Unavailable(m, _) => write!(f, "temporarily unavailable: {m}"),
            ProviderError::Http(code, m) => write!(f, "HTTP {code}: {m}"),
            ProviderError::Network(m) => write!(f, "network error: {m}"),
            ProviderError::Parse(m) => write!(f, "unexpected response: {m}"),
        }
    }
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_decode() {
            ProviderError::Parse(e.to_string())
        } else {
            ProviderError::Network(e.to_string())
        }
    }
}

pub type ProviderResult<T> = Result<T, ProviderError>;

pub trait LyricsProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn ready(&self) -> Result<(), String>;
    fn lookup<'a>(&'a self, q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Option<(Candidate, Lyrics)>>>;
    fn search<'a>(&'a self, q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Vec<Candidate>>>;
    fn fetch<'a>(&'a self, c: &'a Candidate) -> BoxFuture<'a, ProviderResult<Lyrics>>;
}

pub struct ProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
}

pub const KNOWN_PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: local::ID,
        name: local::NAME,
    },
    ProviderInfo {
        id: spicy::ID,
        name: spicy::NAME,
    },
    ProviderInfo {
        id: lrcred::ID,
        name: lrcred::NAME,
    },
    ProviderInfo {
        id: unison::ID,
        name: unison::NAME,
    },
    ProviderInfo {
        id: lrclib::ID,
        name: lrclib::NAME,
    },
];

static GATES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<&'static str, Arc<RateGate>>>> =
    std::sync::LazyLock::new(Default::default);

pub fn gate_for(id: &'static str, min_interval: Duration) -> Arc<RateGate> {
    let mut gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
    gates
        .entry(id)
        .or_insert_with(|| Arc::new(RateGate::new(min_interval)))
        .clone()
}

pub fn provider_name(id: &str) -> &str {
    KNOWN_PROVIDERS
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.name)
        .unwrap_or(id)
}

pub fn cacheable(id: &str) -> bool {
    id != local::ID
}

pub fn ordered_ids(config: &Config) -> Vec<String> {
    let mut ids: Vec<String> = config
        .providers
        .order
        .iter()
        .filter(|id| KNOWN_PROVIDERS.iter().any(|p| p.id == id.as_str()))
        .cloned()
        .collect();
    for (i, p) in KNOWN_PROVIDERS.iter().enumerate() {
        if !ids.iter().any(|x| x == p.id) {
            ids.insert(i.min(ids.len()), p.id.to_string());
        }
    }
    ids
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(12))
        .connect_timeout(Duration::from_secs(6))
        .build()
        .expect("http client")
}

#[derive(Clone)]
pub struct Registry {
    pub providers: Vec<Arc<dyn LyricsProvider>>,
    pub prefer_synced: bool,
    pub prioritize_local: bool,
}

impl Registry {
    pub fn from_config(config: &Config, client: &reqwest::Client) -> Self {
        let mut providers: Vec<Arc<dyn LyricsProvider>> = Vec::new();
        for id in ordered_ids(config) {
            if !config.providers.is_enabled(&id) {
                continue;
            }
            match id.as_str() {
                local::ID => providers.push(Arc::new(local::LocalLibrary::new(
                    client.clone(),
                    crate::library::Library::default(),
                    config.providers.local_library.duration_tolerance_secs,
                ))),
                spicy::ID => providers.push(Arc::new(spicy::SpicyLyrics::new(
                    client.clone(),
                    config.providers.spicy_lyrics.effective_key(),
                    config.providers.spicy_lyrics.min_tier,
                ))),
                lrcred::ID => providers.push(Arc::new(lrcred::LrcRed::new(
                    client.clone(),
                    config.providers.lrc_red.duration_tolerance_secs,
                ))),
                unison::ID => providers.push(Arc::new(unison::Unison::new(
                    client.clone(),
                    config.providers.unison.duration_tolerance_secs,
                ))),
                lrclib::ID => providers.push(Arc::new(lrclib::Lrclib::new(
                    client.clone(),
                    config.providers.lrclib.duration_tolerance_secs,
                ))),
                _ => {}
            }
        }
        Self {
            providers,
            prefer_synced: config.providers.prefer_synced,
            prioritize_local: config.providers.local_library.prioritize,
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn LyricsProvider>> {
        self.providers.iter().find(|p| p.id() == id).cloned()
    }
}

pub fn retry_after(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}

pub fn rate_tier(q: &TrackQuery, title: &str, artists: &[String], duration: Option<f64>, tolerance: f64) -> MatchTier {
    let want_title = normalize(&clean_title(&q.title));
    let got_title = normalize(&clean_title(title));
    let title_match = !want_title.is_empty() && (want_title == got_title);
    let title_partial = !title_match
        && !want_title.is_empty()
        && (got_title.contains(&want_title) || want_title.contains(&got_title));
    let want_artists: Vec<String> = q.artists.iter().map(|a| normalize(a)).collect();
    let got_artists: Vec<String> = artists.iter().map(|a| normalize(a)).collect();
    let artist_match = want_artists.is_empty()
        || want_artists.iter().any(|w| {
            got_artists
                .iter()
                .any(|g| !w.is_empty() && (g == w || g.contains(w.as_str()) || w.contains(g.as_str())))
        });
    let duration_delta = match (q.duration, duration) {
        (Some(a), Some(b)) => Some((a - b).abs()),
        _ => None,
    };
    if title_match {
        if !artist_match {
            return MatchTier::OtherArtist;
        }
        match duration_delta {
            Some(d) if d <= tolerance => {
                if want_artists.is_empty() {
                    MatchTier::Likely
                } else {
                    MatchTier::Exact
                }
            }
            Some(d) if d <= tolerance * 4.0 => MatchTier::Likely,
            Some(_) => MatchTier::Version,
            None => MatchTier::Likely,
        }
    } else if title_partial && artist_match {
        MatchTier::Version
    } else {
        MatchTier::Related
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn live_lookup() {
        let client = http_client();
        let q = TrackQuery {
            title: "Call Back".into(),
            artists: vec!["Don Toliver".into()],
            album: Some("OCTANE".into()),
            duration: Some(123.84),
            ..Default::default()
        };
        let providers: Vec<Box<dyn LyricsProvider>> = vec![
            Box::new(lrcred::LrcRed::new(client.clone(), 2.0)),
            Box::new(unison::Unison::new(client.clone(), 2.0)),
            Box::new(lrclib::Lrclib::new(client.clone(), 2.0)),
        ];
        for p in providers {
            match p.lookup(&q).await {
                Ok(Some((c, l))) => println!(
                    "{}: {} via {} ({:?}), {} lines, bg groups {}, credit url {:?}",
                    p.name(),
                    l.kind.label(),
                    c.id,
                    c.tier,
                    l.lines.len(),
                    l.lines.iter().map(|x| x.background.len()).sum::<usize>(),
                    l.credits.provider_url
                ),
                Ok(None) => println!("{}: no match", p.name()),
                Err(e) => println!("{}: error {e}", p.name()),
            }
            let cands = p.search(&q).await.map(|c| c.len());
            println!("{}: search candidates {:?}", p.name(), cands.map_err(|e| e.to_string()));
        }
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Song - Remastered 2011"), "Song");
        assert_eq!(clean_title("Song (feat. Someone)"), "Song");
        assert_eq!(clean_title("Song (Interlude)"), "Song (Interlude)");
    }

    #[test]
    fn rates_tiers() {
        let q = TrackQuery {
            title: "Call Back".into(),
            artists: vec!["Don Toliver".into()],
            duration: Some(123.8),
            ..Default::default()
        };
        assert_eq!(rate_tier(&q, "Call Back", &["Don Toliver".into()], Some(124.0), 2.0), MatchTier::Exact);
        assert_eq!(rate_tier(&q, "Call Back", &["Someone".into()], Some(124.0), 2.0), MatchTier::OtherArtist);
        assert_eq!(rate_tier(&q, "Call Back", &["Don Toliver".into()], Some(200.0), 2.0), MatchTier::Version);
    }
}
