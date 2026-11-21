use futures::future::BoxFuture;

use super::{Candidate, LyricsProvider, ProviderError, ProviderResult, TrackQuery, rate_tier};
use crate::config::MatchTier;
use crate::library::{Entry, Library};
use crate::lyrics::Lyrics;

pub const ID: &str = "local-library";
pub const NAME: &str = "Local library";

pub struct LocalLibrary {
    client: reqwest::Client,
    library: Library,
    tolerance: f64,
}

impl LocalLibrary {
    pub fn new(client: reqwest::Client, library: Library, tolerance: f64) -> Self {
        Self {
            client,
            library,
            tolerance,
        }
    }

    fn tier(&self, q: &TrackQuery, e: &Entry) -> MatchTier {
        if e.key == q.key() {
            MatchTier::Exact
        } else {
            rate_tier(q, &e.track.title, &e.track.artists, e.track.duration, self.tolerance)
        }
    }

    fn candidate(&self, q: &TrackQuery, e: &Entry) -> Candidate {
        Candidate {
            provider: ID.into(),
            id: e.id.clone(),
            title: e.track.title.clone(),
            artists: e.track.artists.clone(),
            album: e.track.album.clone(),
            duration: e.track.duration,
            tier: Some(self.tier(q, e)),
            kind_hint: e.kind,
            note: Some(format!("{} {}", e.source.label(), e.source.name())),
        }
    }

    fn best(&self, q: &TrackQuery) -> Option<Entry> {
        let key = q.key();
        let entries = self.library.entries();
        if let Some(e) = entries.iter().find(|e| e.key == key) {
            return Some(e.clone());
        }
        entries
            .into_iter()
            .map(|e| (self.tier(q, &e), e))
            .filter(|(t, _)| *t <= MatchTier::Likely)
            .min_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.updated_at.cmp(&a.1.updated_at)))
            .map(|(_, e)| e)
    }

    async fn load(&self, e: &Entry, duration: Option<f64>) -> ProviderResult<Lyrics> {
        self.library.load(e, &self.client, duration).await.map_err(ProviderError::Local)
    }
}

impl LyricsProvider for LocalLibrary {
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
            let Some(e) = self.best(q) else { return Ok(None) };
            let lyrics = self.load(&e, q.duration).await?;
            Ok(Some((self.candidate(q, &e), lyrics)))
        })
    }

    fn search<'a>(&'a self, q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Vec<Candidate>>> {
        Box::pin(async move {
            let mut out: Vec<Candidate> = self.library.entries().iter().map(|e| self.candidate(q, e)).collect();
            out.sort_by(|a, b| a.tier.cmp(&b.tier).then_with(|| a.title.cmp(&b.title)));
            Ok(out)
        })
    }

    fn fetch<'a>(&'a self, c: &'a Candidate) -> BoxFuture<'a, ProviderResult<Lyrics>> {
        Box::pin(async move {
            let e = self.library.get(&c.id).ok_or(ProviderError::NotFound)?;
            self.load(&e, c.duration).await
        })
    }
}
