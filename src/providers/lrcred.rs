use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde::Deserialize;

use super::{
    Candidate, LyricsProvider, ProviderError, ProviderResult, RateGate, TrackQuery, clean_title, gate_for, rate_tier,
    retry_after,
};
use crate::config::MatchTier;
use crate::lyrics::{Credits, Lyrics, SyncKind, ttml};

pub const ID: &str = "lrc-red";
pub const NAME: &str = "lrc.red";
const BASE: &str = "https://lrc.red";
const MAX_LOOKUP_FETCHES: usize = 3;

pub struct LrcRed {
    client: reqwest::Client,
    tolerance: f64,
    gate: Arc<RateGate>,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchPage {
    #[serde(default)]
    hits: Vec<Hit>,
}

#[derive(Debug, Clone, Deserialize)]
struct Hit {
    isrc: String,
    title: String,
    #[serde(default)]
    artist: String,
    album: Option<String>,
    duration: Option<f64>,
    year: Option<i64>,
}

impl Hit {
    fn artists(&self) -> Vec<String> {
        vec![self.artist.clone()]
    }
}

fn valid_isrc(s: &str) -> bool {
    s.len() == 12 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

impl LrcRed {
    pub fn new(client: reqwest::Client, tolerance: f64) -> Self {
        Self {
            client,
            tolerance,
            gate: gate_for(ID, Duration::from_millis(300)),
        }
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> ProviderResult<String> {
        self.gate.acquire().await?;
        let result = self.send(path, query).await;
        self.gate.observe(result).await
    }

    async fn send(&self, path: &str, query: &[(&str, String)]) -> ProviderResult<String> {
        let resp = self.client.get(format!("{BASE}{path}")).query(query).send().await?;
        let status = resp.status().as_u16();
        match status {
            200 => resp.text().await.map_err(Into::into),
            404 => Err(ProviderError::NotFound),
            429 => Err(ProviderError::RateLimited(retry_after(&resp))),
            502..=504 => Err(ProviderError::Unavailable(format!("HTTP {status}"), retry_after(&resp))),
            _ => Err(ProviderError::Http(status, String::new())),
        }
    }

    async fn search_hits(&self, q: &TrackQuery) -> ProviderResult<Vec<Hit>> {
        let mut text = clean_title(&q.title);
        if let Some(a) = q.artists.first() {
            text.push(' ');
            text.push_str(a);
        }
        let body = self.get("/search.json", &[("q", text)]).await?;
        let page: SearchPage = serde_json::from_str(&body).map_err(|e| ProviderError::Parse(e.to_string()))?;
        Ok(page.hits.into_iter().filter(|h| valid_isrc(&h.isrc)).collect())
    }

    async fn lyrics_for(&self, isrc: &str) -> ProviderResult<Lyrics> {
        if !valid_isrc(isrc) {
            return Err(ProviderError::NotFound);
        }
        let doc = self.get(&format!("/s/{isrc}.ttml"), &[]).await?;
        let credits = Credits {
            provider: NAME.into(),
            provider_url: Some(format!("{BASE}/s/{isrc}")),
            source: None,
            source_tag: None,
            songwriters: Vec::new(),
            contributors: Vec::new(),
        };
        ttml::parse(&doc, credits).ok_or(ProviderError::NotFound)
    }

    fn candidate(&self, q: &TrackQuery, h: &Hit) -> Candidate {
        Candidate {
            provider: ID.into(),
            id: h.isrc.clone(),
            title: h.title.clone(),
            artists: h.artists(),
            album: h.album.clone(),
            duration: h.duration,
            tier: Some(rate_tier(q, &h.title, &h.artists(), h.duration, self.tolerance)),
            kind_hint: None,
            note: Some(match h.year {
                Some(y) => format!("{y} · ISRC {}", h.isrc),
                None => format!("ISRC {}", h.isrc),
            }),
        }
    }
}

impl LyricsProvider for LrcRed {
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
            if let Some(isrc) = q.isrc.as_deref().filter(|i| valid_isrc(i)) {
                match self.lyrics_for(isrc).await {
                    Ok(l) => {
                        let mut c = Candidate {
                            provider: ID.into(),
                            id: isrc.to_string(),
                            title: q.title.clone(),
                            artists: q.artists.clone(),
                            album: q.album.clone(),
                            duration: q.duration,
                            tier: Some(MatchTier::Exact),
                            kind_hint: None,
                            note: Some("matched by ISRC".into()),
                        };
                        c.kind_hint = Some(l.kind);
                        return Ok(Some((c, l)));
                    }
                    Err(ProviderError::NotFound) => {}
                    Err(e) => return Err(e),
                }
            }
            if !q.is_searchable() {
                return Ok(None);
            }
            let hits = self.search_hits(q).await?;
            let mut fallback: Option<(Candidate, Lyrics)> = None;
            let exact = hits
                .iter()
                .filter(|h| rate_tier(q, &h.title, &h.artists(), h.duration, self.tolerance) == MatchTier::Exact)
                .take(MAX_LOOKUP_FETCHES);
            for h in exact {
                match self.lyrics_for(&h.isrc).await {
                    Ok(l) => {
                        let mut c = self.candidate(q, h);
                        c.kind_hint = Some(l.kind);
                        if l.kind != SyncKind::Static {
                            return Ok(Some((c, l)));
                        }
                        if fallback.is_none() {
                            fallback = Some((c, l));
                        }
                    }
                    Err(ProviderError::NotFound) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(fallback)
        })
    }

    fn search<'a>(&'a self, q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Vec<Candidate>>> {
        Box::pin(async move {
            if !q.is_searchable() {
                return Ok(Vec::new());
            }
            let mut out: Vec<Candidate> = self.search_hits(q).await?.iter().map(|h| self.candidate(q, h)).collect();
            out.sort_by(|a, b| a.tier.cmp(&b.tier));
            Ok(out)
        })
    }

    fn fetch<'a>(&'a self, c: &'a Candidate) -> BoxFuture<'a, ProviderResult<Lyrics>> {
        Box::pin(async move { self.lyrics_for(&c.id).await })
    }
}
