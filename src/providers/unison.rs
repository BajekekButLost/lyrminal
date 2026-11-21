use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;

use super::{
    Candidate, LyricsProvider, ProviderError, ProviderResult, RateGate, TrackQuery, clean_title, gate_for, rate_tier,
    retry_after,
};
use crate::config::MatchTier;
use crate::lyrics::formats::{self, Format};
use crate::lyrics::{Contributor, Credits, Lyrics, SyncKind};

pub const ID: &str = "unison";
pub const NAME: &str = "Better Lyrics";
pub const CREDIT: &str = "Lyrics from Unison";
const BASE: &str = "https://unison.betterlyrics.org";
const SEARCH_LIMIT: &str = "10";

pub struct Unison {
    client: reqwest::Client,
    tolerance: f64,
    gate: Arc<RateGate>,
}

static ETAGS: std::sync::LazyLock<Mutex<HashMap<String, (String, Lyrics)>>> = std::sync::LazyLock::new(Default::default);

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    success: bool,
    data: Option<T>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    video_id: String,
    song: String,
    #[serde(default)]
    artist: String,
    album: Option<String>,
    duration: Option<f64>,
    sync_type: Option<String>,
    confidence: Option<String>,
    #[serde(default)]
    match_score: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    video_id: String,
    lyrics: String,
    format: String,
    language: Option<String>,
    submitter: Option<Submitter>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Submitter {
    key_id: Option<String>,
    display_name: Option<String>,
}

fn split_artists(artist: &str) -> Vec<String> {
    artist
        .split([',', '&'])
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect()
}

fn sync_kind(sync_type: Option<&str>) -> Option<SyncKind> {
    Some(match sync_type? {
        "richsync" => SyncKind::Syllable,
        "linesync" => SyncKind::Line,
        "plain" => SyncKind::Static,
        _ => return None,
    })
}

fn format_of(format: &str) -> Option<Format> {
    Some(match format {
        "ttml" => Format::Ttml,
        "lrc" => Format::Lrc,
        "plain" => Format::Text,
        _ => return None,
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn song_url(video_id: &str) -> String {
    format!("{BASE}/song/{video_id}")
}

fn credits(record: &Record) -> Credits {
    let contributors = record
        .submitter
        .as_ref()
        .and_then(|s| {
            let name = s.display_name.as_deref().map(str::trim).filter(|n| !n.is_empty())?;
            Some(Contributor {
                role: "submitted by".into(),
                name: name.to_string(),
                url: s
                    .key_id
                    .as_deref()
                    .filter(|k| valid_id(k))
                    .map(|k| format!("{BASE}/curator/{k}")),
            })
        })
        .into_iter()
        .collect();
    Credits {
        provider: NAME.into(),
        provider_url: Some(song_url(&record.video_id)),
        provider_credit: Some(CREDIT.into()),
        source: None,
        source_tag: None,
        songwriters: Vec::new(),
        contributors,
    }
}

fn convert(record: Record, duration: Option<f64>) -> ProviderResult<Lyrics> {
    let format = format_of(&record.format);
    let credits = credits(&record);
    let (mut lyrics, _) = formats::parse(&record.lyrics, format, None, credits, duration)
        .map_err(|_| ProviderError::Parse(format!("could not read {} lyrics", record.format)))?;
    if lyrics.language.is_none() {
        lyrics.language = record.language;
    }
    Ok(lyrics)
}

impl Unison {
    pub fn new(client: reqwest::Client, tolerance: f64) -> Self {
        Self {
            client,
            tolerance,
            gate: gate_for(ID, Duration::from_millis(150)),
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)], etag: Option<&str>) -> ProviderResult<Option<(T, Option<String>)>> {
        self.gate.acquire().await?;
        let result = self.send(path, query, etag).await;
        self.gate.observe(result).await
    }

    async fn send<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)], etag: Option<&str>) -> ProviderResult<Option<(T, Option<String>)>> {
        let mut req = self.client.get(format!("{BASE}{path}")).query(query);
        if let Some(tag) = etag {
            req = req.header(reqwest::header::IF_NONE_MATCH, tag);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let tag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        match status {
            200 => {
                let body: Envelope<T> = resp.json().await?;
                match (body.success, body.data) {
                    (true, Some(data)) => Ok(Some((data, tag))),
                    (true, None) => Err(ProviderError::NotFound),
                    (false, _) => Err(ProviderError::Http(status, body.error.unwrap_or_default())),
                }
            }
            304 => Ok(None),
            404 => Err(ProviderError::NotFound),
            429 => Err(ProviderError::RateLimited(retry_after(&resp))),
            502..=504 => Err(ProviderError::Unavailable(format!("HTTP {status}"), retry_after(&resp))),
            _ => {
                let body: Option<Envelope<serde_json::Value>> = resp.json().await.ok();
                Err(ProviderError::Http(status, body.and_then(|b| b.error).unwrap_or_default()))
            }
        }
    }

    async fn search_hits(&self, q: &TrackQuery) -> ProviderResult<Vec<Hit>> {
        let mut text = clean_title(&q.title);
        if let Some(a) = q.artists.first() {
            text.push(' ');
            text.push_str(a);
        }
        let query = [("q", text), ("limit", SEARCH_LIMIT.to_string())];
        let hits: Vec<Hit> = match self.get("/lyrics/search", &query, None).await {
            Ok(Some((hits, _))) => hits,
            Ok(None) | Err(ProviderError::NotFound) => Vec::new(),
            Err(e) => return Err(e),
        };
        let mut out: Vec<Hit> = Vec::new();
        for h in hits.into_iter().filter(|h| valid_id(&h.video_id)) {
            if !out.iter().any(|o| o.video_id == h.video_id) {
                out.push(h);
            }
        }
        Ok(out)
    }

    async fn lyrics_for(&self, video_id: &str, duration: Option<f64>) -> ProviderResult<Lyrics> {
        if !valid_id(video_id) {
            return Err(ProviderError::NotFound);
        }
        let known = ETAGS.lock().await.get(video_id).cloned();
        let query = [("v", video_id.to_string())];
        let fetched = self
            .get::<Record>("/lyrics", &query, known.as_ref().map(|(tag, _)| tag.as_str()))
            .await;
        match fetched {
            Ok(Some((record, tag))) => {
                let lyrics = convert(record, duration)?;
                if let Some(tag) = tag {
                    ETAGS.lock().await.insert(video_id.to_string(), (tag, lyrics.clone()));
                }
                Ok(lyrics)
            }
            Ok(None) => known.map(|(_, l)| l).ok_or(ProviderError::NotFound),
            Err(e) => {
                if matches!(e, ProviderError::NotFound) {
                    ETAGS.lock().await.remove(video_id);
                }
                Err(e)
            }
        }
    }

    fn tier(&self, q: &TrackQuery, h: &Hit) -> MatchTier {
        rate_tier(q, &h.song, &split_artists(&h.artist), h.duration, self.tolerance)
    }

    fn candidate(&self, q: &TrackQuery, h: &Hit) -> Candidate {
        let mut note = Vec::new();
        if let Some(c) = h.confidence.as_deref().filter(|c| !c.is_empty()) {
            note.push(format!("{c} confidence"));
        }
        note.push(format!("{:.0}% match", h.match_score.clamp(0.0, 1.0) * 100.0));
        Candidate {
            provider: ID.into(),
            id: h.video_id.clone(),
            title: h.song.clone(),
            artists: split_artists(&h.artist),
            album: h.album.clone(),
            duration: h.duration,
            tier: Some(self.tier(q, h)),
            kind_hint: sync_kind(h.sync_type.as_deref()),
            note: Some(note.join(" · ")),
        }
    }
}

fn best_exact<'a>(hits: impl Iterator<Item = &'a Hit>) -> Option<&'a Hit> {
    let mut best: Option<&Hit> = None;
    for h in hits {
        let rank = |x: &Hit| sync_kind(x.sync_type.as_deref()).map(|k| k.rank()).unwrap_or(0);
        if best.is_none_or(|b| rank(h) > rank(b)) {
            best = Some(h);
        }
    }
    best
}

impl LyricsProvider for Unison {
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
            let hits = self.search_hits(q).await?;
            let Some(h) = best_exact(hits.iter().filter(|h| self.tier(q, h) == MatchTier::Exact)) else {
                return Ok(None);
            };
            match self.lyrics_for(&h.video_id, q.duration).await {
                Ok(l) => {
                    let mut c = self.candidate(q, h);
                    c.kind_hint = Some(l.kind);
                    Ok(Some((c, l)))
                }
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
            let mut out: Vec<Candidate> = self.search_hits(q).await?.iter().map(|h| self.candidate(q, h)).collect();
            out.sort_by_key(|c| c.tier);
            Ok(out)
        })
    }

    fn fetch<'a>(&'a self, c: &'a Candidate) -> BoxFuture<'a, ProviderResult<Lyrics>> {
        Box::pin(async move { self.lyrics_for(&c.id, c.duration).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(video_id: &str, sync_type: &str) -> Hit {
        Hit {
            video_id: video_id.into(),
            song: "Explorer".into(),
            artist: "Justice, Connan Mockasin".into(),
            album: None,
            duration: Some(249.0),
            sync_type: Some(sync_type.into()),
            confidence: None,
            match_score: 1.0,
        }
    }

    #[tokio::test]
    #[ignore]
    async fn live_lookup() {
        let u = Unison::new(super::super::http_client(), 2.0);
        let q = TrackQuery {
            title: "Explorer".into(),
            artists: vec!["Justice".into(), "Connan Mockasin".into()],
            duration: Some(249.0),
            ..Default::default()
        };
        let (c, l) = u.lookup(&q).await.unwrap().unwrap();
        println!("{c:?} {} lines {:?}", l.lines.len(), l.credits);
        assert_eq!(l.kind, SyncKind::Syllable);
        assert!(!l.lines.is_empty());
        assert!(!l.credits.contributors.is_empty());
        let again = u.fetch(&c).await.unwrap();
        assert_eq!(again.lines.len(), l.lines.len());
    }

    #[test]
    fn splits_credited_artists() {
        assert_eq!(split_artists("Justice, Connan Mockasin & SZA"), vec!["Justice", "Connan Mockasin", "SZA"]);
    }

    #[test]
    fn prefers_the_best_sync_among_exact_hits() {
        let hits = [hit("a", "linesync"), hit("b", "richsync"), hit("c", "richsync")];
        assert_eq!(best_exact(hits.iter()).map(|h| h.video_id.as_str()), Some("b"));
    }

    #[test]
    fn reads_search_results() {
        let body = r#"{"success":true,"data":[{"id":1,"videoId":"Ugr4KKK_SqY","song":"Explorer","artist":"Justice, Connan Mockasin","duration":249,"format":"ttml","syncType":"richsync","confidence":"high","matchScore":0.98}]}"#;
        let env: Envelope<Vec<Hit>> = serde_json::from_str(body).unwrap();
        let hits = env.data.unwrap();
        let u = Unison::new(reqwest::Client::new(), 2.0);
        let q = TrackQuery {
            title: "Explorer".into(),
            artists: vec!["Justice".into()],
            duration: Some(249.5),
            ..Default::default()
        };
        let c = u.candidate(&q, &hits[0]);
        assert_eq!(c.tier, Some(MatchTier::Exact));
        assert_eq!(c.kind_hint, Some(SyncKind::Syllable));
        assert_eq!(c.note.as_deref(), Some("high confidence · 98% match"));
    }

    #[test]
    fn credits_unison_and_the_submitter() {
        let record = Record {
            video_id: "Ugr4KKK_SqY".into(),
            lyrics: "[00:01.00]Hello\n[00:02.00]World".into(),
            format: "lrc".into(),
            language: Some("en".into()),
            submitter: Some(Submitter {
                key_id: Some("a80e8e35".into()),
                display_name: Some("boidu".into()),
            }),
        };
        let l = convert(record, None).unwrap();
        assert_eq!(l.kind, SyncKind::Line);
        assert_eq!(l.credits.provider_credit.as_deref(), Some(CREDIT));
        assert_eq!(l.credits.provider_url.as_deref(), Some("https://unison.betterlyrics.org/song/Ugr4KKK_SqY"));
        assert_eq!(l.credits.contributors[0].name, "boidu");
        assert_eq!(
            l.credits.contributors[0].url.as_deref(),
            Some("https://unison.betterlyrics.org/curator/a80e8e35")
        );
    }
}
