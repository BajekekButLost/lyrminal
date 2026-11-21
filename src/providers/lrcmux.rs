use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::OnceCell;

use super::{
    Candidate, LyricsProvider, ProviderError, ProviderResult, RateGate, TrackQuery, clean_title, gate_for, rate_tier,
    retry_after,
};
use crate::config::MatchTier;
use crate::lyrics::{Credits, LyricLine, Lyrics, Syllable, SyncKind, VocalGroup};

pub const ID: &str = "lrcmux";
pub const NAME: &str = "lrcmux";
pub const URL: &str = "https://lrcmux.dev";
const BASE: &str = "https://api.lrcmux.dev";
const FALLBACK_SOURCES: &[&str] = &["deezer", "kugou", "ytmusic"];

pub struct Lrcmux {
    client: reqwest::Client,
    tolerance: f64,
    gate: Arc<RateGate>,
}

static SOURCES: OnceCell<Vec<String>> = OnceCell::const_new();

#[derive(Debug, Deserialize)]
struct Problem {
    #[serde(default)]
    detail: String,
}

#[derive(Debug, Deserialize)]
struct SourceInfo {
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Response {
    track: Track,
    meta: Meta,
    #[serde(default)]
    lines: Option<Vec<Line>>,
}

#[derive(Debug, Clone, Deserialize)]
struct Track {
    #[serde(default)]
    isrc: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: String,
    #[serde(default)]
    duration: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct Meta {
    #[serde(default)]
    instrumental: bool,
    level: String,
    source: Option<Source>,
}

#[derive(Debug, Clone, Deserialize)]
struct Source {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Line {
    #[serde(default)]
    text: String,
    start: Option<i64>,
    end: Option<i64>,
    #[serde(default)]
    words: Option<Vec<Word>>,
}

#[derive(Debug, Clone, Deserialize)]
struct Word {
    #[serde(default)]
    text: String,
    start: i64,
    end: i64,
}

fn sync_kind(level: &str) -> Option<SyncKind> {
    Some(match level {
        "word" => SyncKind::Syllable,
        "line" => SyncKind::Line,
        "none" => SyncKind::Static,
        _ => return None,
    })
}

fn valid_isrc(isrc: &str) -> bool {
    isrc.len() == 12
        && isrc[..2].chars().all(|c| c.is_ascii_uppercase())
        && isrc[2..5].chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && isrc[5..].chars().all(|c| c.is_ascii_digit())
}

fn valid_source(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn split_id(id: &str) -> (&str, &str) {
    id.split_once('/').unwrap_or((id, ""))
}

fn split_artists(artist: &str) -> Vec<String> {
    artist
        .split([',', '&'])
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect()
}

fn credits(source: Option<&Source>) -> Credits {
    let name = source.map(|s| s.name.trim().to_string()).filter(|n| !n.is_empty());
    Credits {
        provider: NAME.into(),
        provider_url: Some(URL.into()),
        provider_credit: None,
        source: Some(name.clone().unwrap_or_else(|| "Unknown source".into())),
        source_tag: name,
        songwriters: Vec::new(),
        contributors: Vec::new(),
    }
}

fn secs(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

fn convert(resp: &Response) -> ProviderResult<Lyrics> {
    let credits = credits(resp.meta.source.as_ref());
    if resp.meta.instrumental {
        return Ok(Lyrics {
            kind: SyncKind::Static,
            lines: Vec::new(),
            credits,
            language: None,
            instrumental: true,
        });
    }
    let kind = sync_kind(&resp.meta.level).ok_or_else(|| ProviderError::Parse(format!("unknown sync level {}", resp.meta.level)))?;
    let raw = resp.lines.as_deref().unwrap_or_default();
    let duration = track_duration(resp);
    let mut lines = Vec::new();
    let mut words_seen = false;
    for (i, l) in raw.iter().enumerate() {
        let words = l.words.as_deref().unwrap_or_default();
        if l.text.trim().is_empty() && words.is_empty() {
            continue;
        }
        if kind == SyncKind::Static {
            lines.push(LyricLine {
                start: 0.0,
                end: 0.0,
                opposite: false,
                lead: VocalGroup::single(l.text.trim().to_string(), 0.0, 0.0),
                background: Vec::new(),
                background_only: false,
            });
            continue;
        }
        let Some(start) = l.start.map(secs) else { continue };
        let next = raw[i + 1..].iter().find_map(|n| n.start).map(secs);
        let end = l.end.map(secs).or(next).or(duration).unwrap_or(start + 5.0).max(start);
        let mut syllables: Vec<Syllable> = Vec::new();
        if kind == SyncKind::Syllable {
            for (wi, w) in words.iter().enumerate() {
                let trimmed = w.text.trim();
                if trimmed.is_empty() {
                    if let Some(last) = syllables.last_mut() {
                        last.part_of_word = false;
                    }
                    continue;
                }
                let ws = secs(w.start);
                syllables.push(Syllable {
                    text: trimmed.to_string(),
                    start: ws,
                    end: secs(w.end).max(ws),
                    part_of_word: !w.text.ends_with(char::is_whitespace) && wi + 1 < words.len(),
                    roman: None,
                    roman_part_of_word: false,
                });
            }
        }
        words_seen |= !syllables.is_empty();
        let lead = if syllables.is_empty() {
            VocalGroup::single(l.text.trim().to_string(), start, end)
        } else {
            VocalGroup {
                start,
                end,
                syllables,
                roman: None,
                translation: None,
            }
        };
        let mut line = LyricLine {
            start,
            end,
            opposite: false,
            lead,
            background: Vec::new(),
            background_only: false,
        };
        line.normalize(true);
        lines.push(line);
    }
    if lines.is_empty() {
        return Err(ProviderError::NotFound);
    }
    let kind = if kind == SyncKind::Syllable && !words_seen { SyncKind::Line } else { kind };
    Ok(Lyrics {
        kind,
        lines,
        credits,
        language: None,
        instrumental: false,
    })
}

impl Lrcmux {
    pub fn new(client: reqwest::Client, tolerance: f64) -> Self {
        Self {
            client,
            tolerance,
            gate: gate_for(ID, Duration::from_millis(200)),
        }
    }

    async fn request<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> ProviderResult<T> {
        self.gate.acquire().await?;
        let result = self.send(path, query).await;
        self.gate.observe(result).await
    }

    async fn send<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> ProviderResult<T> {
        let resp = self.client.get(format!("{BASE}{path}")).query(query).send().await?;
        let status = resp.status().as_u16();
        match status {
            200 => resp.json::<T>().await.map_err(Into::into),
            404 => Err(ProviderError::NotFound),
            429 => Err(ProviderError::RateLimited(retry_after(&resp))),
            502..=504 => {
                let retry = retry_after(&resp);
                let detail = resp.json::<Problem>().await.map(|p| p.detail).unwrap_or_default();
                Err(ProviderError::Unavailable(
                    if detail.is_empty() { format!("HTTP {status}") } else { detail },
                    retry,
                ))
            }
            _ => Err(ProviderError::Http(
                status,
                resp.json::<Problem>().await.map(|p| p.detail).unwrap_or_default(),
            )),
        }
    }

    async fn sources(&self) -> Vec<String> {
        let fetched = SOURCES
            .get_or_try_init(|| async {
                let list: Vec<SourceInfo> = self.request("/sources", &[]).await?;
                let ids: Vec<String> = list.into_iter().map(|s| s.id).filter(|id| valid_source(id)).collect();
                if ids.is_empty() { Err(ProviderError::NotFound) } else { Ok(ids) }
            })
            .await;
        match fetched {
            Ok(ids) => ids.clone(),
            Err(_) => FALLBACK_SOURCES.iter().map(|s| s.to_string()).collect(),
        }
    }

    async fn search_one(&self, q: &TrackQuery, source: Option<&str>) -> ProviderResult<Option<Response>> {
        let Some(query) = search_query(q, source) else {
            return Ok(None);
        };
        match self.request::<Response>("/get", &query).await {
            Ok(r) => Ok(Some(r)),
            Err(ProviderError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn tier(&self, q: &TrackQuery, r: &Response) -> MatchTier {
        rate_tier(q, &r.track.title, &split_artists(&r.track.artist), track_duration(r), self.tolerance)
    }

    fn candidate(&self, q: &TrackQuery, r: &Response, lyrics: Option<&Lyrics>) -> Candidate {
        let source = r.meta.source.as_ref();
        let isrc = if valid_isrc(&r.track.isrc) { r.track.isrc.as_str() } else { "" };
        Candidate {
            provider: ID.into(),
            id: format!("{}/{isrc}", source.map(|s| s.id.as_str()).unwrap_or_default()),
            title: r.track.title.clone(),
            artists: split_artists(&r.track.artist),
            album: Some(r.track.album.clone()).filter(|a| !a.is_empty()),
            duration: track_duration(r),
            tier: Some(self.tier(q, r)),
            kind_hint: lyrics.map(|l| l.kind).or_else(|| sync_kind(&r.meta.level)),
            note: source.map(|s| format!("via {}", s.name)),
        }
    }
}

fn track_duration(r: &Response) -> Option<f64> {
    (r.track.duration > 0).then_some(r.track.duration as f64)
}

fn search_query(q: &TrackQuery, source: Option<&str>) -> Option<Vec<(&'static str, String)>> {
    let artist = q.artists.first().map(|a| a.trim()).filter(|a| !a.is_empty())?;
    let title = clean_title(&q.title);
    if title.is_empty() {
        return None;
    }
    let mut query = vec![("artist", artist.to_string()), ("title", title)];
    if let Some(album) = q.album.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        query.push(("album", album.to_string()));
    }
    if let Some(d) = q.duration.filter(|d| (1.0..=3600.0).contains(d)) {
        query.push(("duration", format!("{}", d.round() as i64)));
    }
    if let Some(s) = source {
        query.push(("sources", s.to_string()));
    }
    Some(query)
}

impl LyricsProvider for Lrcmux {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    fn ready(&self) -> Result<(), String> {
        Ok(())
    }

    fn lookup<'a>(&'a self, q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Option<(Candidate, Lyrics)>>> {
        Box::pin(async move {
            if !q.is_searchable() {
                return Ok(None);
            }
            let Some(r) = self.search_one(q, None).await? else {
                return Ok(None);
            };
            if self.tier(q, &r) > MatchTier::Likely {
                return Ok(None);
            }
            match convert(&r) {
                Ok(l) => Ok(Some((self.candidate(q, &r, Some(&l)), l))),
                Err(ProviderError::NotFound) => Ok(None),
                Err(e) => Err(e),
            }
        })
    }

    fn search<'a>(&'a self, q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Vec<Candidate>>> {
        Box::pin(async move {
            if !q.is_searchable() {
                return Ok(Vec::new());
            }
            let sources = self.sources().await;
            let results = futures::future::join_all(sources.iter().map(|s| self.search_one(q, Some(s)))).await;
            let mut out: Vec<Candidate> = Vec::new();
            let mut error = None;
            for result in results {
                match result {
                    Ok(Some(r)) => {
                        let c = self.candidate(q, &r, None);
                        if !out.iter().any(|o| o.id == c.id) {
                            out.push(c);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => error = error.or(Some(e)),
                }
            }
            if out.is_empty()
                && let Some(e) = error
            {
                return Err(e);
            }
            out.sort_by(|a, b| {
                a.tier
                    .cmp(&b.tier)
                    .then_with(|| b.kind_hint.map(|k| k.rank()).cmp(&a.kind_hint.map(|k| k.rank())))
            });
            Ok(out)
        })
    }

    fn fetch<'a>(&'a self, c: &'a Candidate) -> BoxFuture<'a, ProviderResult<Lyrics>> {
        Box::pin(async move {
            let (source, isrc) = split_id(&c.id);
            let source = Some(source).filter(|s| valid_source(s));
            let r = if valid_isrc(isrc) {
                let mut query = vec![("isrc", isrc.to_string())];
                if let Some(s) = source {
                    query.push(("sources", s.to_string()));
                }
                self.request::<Response>("/get", &query).await?
            } else {
                let q = TrackQuery {
                    title: c.title.clone(),
                    artists: c.artists.clone(),
                    album: c.album.clone(),
                    duration: c.duration,
                    ..Default::default()
                };
                self.search_one(&q, source).await?.ok_or(ProviderError::NotFound)?
            };
            convert(&r)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORD: &str = r#"{"track":{"cover":{},"isrc":"FR8EU2300090","title":"Explorer","artist":"Justice","album":"Hyperdrama","duration":249},"meta":{"source":{"id":"deezer","name":"Deezer","url":"https://www.deezer.com"},"level":"word"},"lines":[{"text":"Step out","start":150112,"end":151525,"words":[{"text":"Step ","start":150112,"end":150337},{"text":"out","start":150475,"end":151525}]},{"text":"","start":152000,"end":152000},{"text":"Nothing","start":153825,"end":157075,"words":[{"text":"Noth","start":153825,"end":154000},{"text":"ing","start":154000,"end":157075}]}]}"#;

    fn query() -> TrackQuery {
        TrackQuery {
            title: "Explorer".into(),
            artists: vec!["Justice".into(), "Connan Mockasin".into()],
            duration: Some(249.4),
            ..Default::default()
        }
    }

    #[test]
    fn reads_word_synced_lyrics() {
        let r: Response = serde_json::from_str(WORD).unwrap();
        let l = convert(&r).unwrap();
        assert_eq!(l.kind, SyncKind::Syllable);
        assert_eq!(l.lines.len(), 2);
        let s = &l.lines[0].lead.syllables;
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "Step");
        assert!(!s[0].part_of_word);
        assert!((s[1].end - 151.525).abs() < 1e-9);
        let s = &l.lines[1].lead.syllables;
        assert!(s[0].part_of_word);
        assert!(!s[1].part_of_word);
    }

    #[test]
    fn credits_lrcmux_and_names_the_source() {
        let r: Response = serde_json::from_str(WORD).unwrap();
        let l = convert(&r).unwrap();
        assert_eq!(l.credits.provider, NAME);
        assert_eq!(l.credits.provider_url.as_deref(), Some(URL));
        assert_eq!(l.credits.source.as_deref(), Some("Deezer"));
        assert_eq!(l.credits.source_tag.as_deref(), Some("Deezer"));
        let unknown = credits(None);
        assert_eq!(unknown.source.as_deref(), Some("Unknown source"));
        assert_eq!(unknown.source_tag, None);
    }

    #[test]
    fn reads_line_and_plain_lyrics() {
        let line = r#"{"track":{"cover":{},"isrc":"","title":"A","artist":"B","album":"","duration":0},"meta":{"source":{"id":"ytmusic","name":"YouTube Music"},"level":"line"},"lines":[{"text":"one","start":1000,"end":2000},{"text":"two","start":2000}]}"#;
        let r: Response = serde_json::from_str(line).unwrap();
        let l = convert(&r).unwrap();
        assert_eq!(l.kind, SyncKind::Line);
        assert!((l.lines[1].end - 7.0).abs() < 1e-9);
        let plain = r#"{"track":{"cover":{},"isrc":"","title":"A","artist":"B","album":"","duration":0},"meta":{"level":"none"},"lines":[{"text":"one"},{"text":""},{"text":"two"}]}"#;
        let r: Response = serde_json::from_str(plain).unwrap();
        let l = convert(&r).unwrap();
        assert_eq!(l.kind, SyncKind::Static);
        assert_eq!(l.lines.len(), 2);
    }

    #[test]
    fn builds_candidates_keyed_by_source_and_isrc() {
        let r: Response = serde_json::from_str(WORD).unwrap();
        let m = Lrcmux::new(reqwest::Client::new(), 2.0);
        let c = m.candidate(&query(), &r, None);
        assert_eq!(c.id, "deezer/FR8EU2300090");
        assert_eq!(split_id(&c.id), ("deezer", "FR8EU2300090"));
        assert_eq!(c.tier, Some(MatchTier::Exact));
        assert_eq!(c.kind_hint, Some(SyncKind::Syllable));
        assert_eq!(c.note.as_deref(), Some("via Deezer"));
        assert_eq!(c.album.as_deref(), Some("Hyperdrama"));
    }

    #[test]
    fn searches_by_title_and_first_artist() {
        let mut q = query();
        q.title = "Explorer (feat. Connan Mockasin)".into();
        q.isrc = Some("FR8EU2300090".into());
        let s = search_query(&q, Some("kugou")).unwrap();
        assert_eq!(
            s,
            vec![
                ("artist", "Justice".to_string()),
                ("title", "Explorer".to_string()),
                ("duration", "249".to_string()),
                ("sources", "kugou".to_string()),
            ]
        );
        q.artists.clear();
        assert!(search_query(&q, None).is_none());
    }

    #[test]
    fn validates_isrcs() {
        assert!(valid_isrc("FR8EU2300090"));
        assert!(!valid_isrc("fr8eu2300090"));
        assert!(!valid_isrc("FR8EU230009"));
        assert!(!valid_isrc(""));
    }

    #[tokio::test]
    #[ignore]
    async fn live_lookup() {
        let m = Lrcmux::new(super::super::http_client(), 2.0);
        let (c, l) = m.lookup(&query()).await.unwrap().unwrap();
        println!("{c:?} {} lines {:?}", l.lines.len(), l.credits);
        assert_eq!(l.kind, SyncKind::Syllable);
        let again = m.fetch(&c).await.unwrap();
        assert_eq!(again.lines.len(), l.lines.len());
        let found = m.search(&query()).await.unwrap();
        println!("{found:?}");
        assert!(found.len() >= 2);
    }
}
