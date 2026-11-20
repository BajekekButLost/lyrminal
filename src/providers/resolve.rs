use futures::future::join_all;

use super::cache::Cache;
use super::{Candidate, ProviderError, ProviderResult, Registry, TrackQuery};
use crate::lyrics::{Lyrics, SyncKind};

#[derive(Debug, Clone)]
pub struct Resolution {
    pub candidate: Candidate,
    pub lyrics: Lyrics,
    pub manual: bool,
}

#[derive(Debug, Clone)]
pub struct Attempt {
    pub provider: String,
    pub outcome: String,
}

#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub result: Option<Resolution>,
    pub attempts: Vec<Attempt>,
}

pub async fn fetch_candidate(registry: &Registry, cache: &Cache, c: &Candidate, bypass_cache: bool) -> ProviderResult<Lyrics> {
    if !bypass_cache && let Some(l) = cache.get_lyrics(&c.provider, &c.id) {
        return Ok(l);
    }
    let provider = registry
        .get(&c.provider)
        .ok_or_else(|| ProviderError::NotConfigured(format!("{} is disabled", super::provider_name(&c.provider))))?;
    let lyrics = provider.fetch(c).await?;
    cache.put_lyrics(&c.provider, &c.id, &lyrics);
    Ok(lyrics)
}

pub async fn resolve(
    registry: &Registry,
    cache: &Cache,
    q: &TrackQuery,
    choice: Option<Candidate>,
    bypass_cache: bool,
) -> Outcome {
    let mut out = Outcome::default();
    if let Some(c) = choice {
        match fetch_candidate(registry, cache, &c, bypass_cache).await {
            Ok(lyrics) => {
                out.result = Some(Resolution {
                    candidate: c,
                    lyrics,
                    manual: true,
                });
                return out;
            }
            Err(e) => out.attempts.push(Attempt {
                provider: super::provider_name(&c.provider).to_string(),
                outcome: format!("chosen sync failed: {e}"),
            }),
        }
    }
    let key = q.key();
    let mut fallback: Option<Resolution> = None;
    for p in &registry.providers {
        if let Err(reason) = p.ready() {
            out.attempts.push(Attempt {
                provider: p.name().into(),
                outcome: format!("skipped: {reason}"),
            });
            continue;
        }
        let cached = if bypass_cache { None } else { cache.get_lookup(p.id(), &key) };
        let looked = match cached {
            Some(hit) => Ok(hit),
            None => {
                let r = p.lookup(q).await;
                if let Ok(v) = &r {
                    cache.put_lookup(p.id(), &key, v);
                }
                r
            }
        };
        match looked {
            Ok(Some((candidate, lyrics))) => {
                out.attempts.push(Attempt {
                    provider: p.name().into(),
                    outcome: if lyrics.instrumental {
                        "instrumental".into()
                    } else {
                        lyrics.kind.label().into()
                    },
                });
                let res = Resolution {
                    candidate,
                    lyrics,
                    manual: false,
                };
                let weak = res.lyrics.kind == SyncKind::Static || res.lyrics.instrumental;
                if !registry.prefer_synced || !weak {
                    out.result = Some(res);
                    return out;
                }
                if fallback.is_none() {
                    fallback = Some(res);
                }
            }
            Ok(None) => out.attempts.push(Attempt {
                provider: p.name().into(),
                outcome: "no match".into(),
            }),
            Err(e) => out.attempts.push(Attempt {
                provider: p.name().into(),
                outcome: e.to_string(),
            }),
        }
    }
    out.result = fallback;
    out
}

pub async fn search_all(registry: &Registry, q: &TrackQuery) -> Vec<(String, ProviderResult<Vec<Candidate>>)> {
    let futures = registry.providers.iter().map(|p| {
        let p = p.clone();
        let q = q.clone();
        async move {
            let r = match p.ready() {
                Ok(()) => p.search(&q).await,
                Err(reason) => Err(ProviderError::NotConfigured(reason)),
            };
            (p.id().to_string(), r)
        }
    });
    join_all(futures).await
}
