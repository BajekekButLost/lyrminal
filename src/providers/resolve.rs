use super::cache::Cache;
use super::{Candidate, ProviderError, ProviderResult, Registry, TrackQuery};
use crate::config::MatchTier;
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
    let cacheable = super::cacheable(&c.provider);
    if cacheable
        && !bypass_cache
        && let Some(l) = cache.get_lyrics(&c.provider, &c.id)
    {
        return Ok(l);
    }
    let provider = registry
        .get(&c.provider)
        .ok_or_else(|| ProviderError::NotConfigured(format!("{} is disabled", super::provider_name(&c.provider))))?;
    let lyrics = provider.fetch(c).await?;
    if cacheable {
        cache.put_lyrics(&c.provider, &c.id, &lyrics);
    }
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
    let mut best: Option<Resolution> = None;
    for p in &registry.providers {
        if let Err(reason) = p.ready() {
            out.attempts.push(Attempt {
                provider: p.name().into(),
                outcome: format!("skipped: {reason}"),
            });
            continue;
        }
        let cacheable = super::cacheable(p.id());
        let cached = if bypass_cache || !cacheable { None } else { cache.get_lookup(p.id(), &key) };
        let looked = match cached {
            Some(hit) => Ok(hit),
            None => {
                let r = p.lookup(q).await;
                if cacheable && let Ok(v) = &r {
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
                if !registry.prefer_synced {
                    out.result = Some(res);
                    return out;
                }
                let trusted = trusted_local(registry, &res.candidate);
                if best.as_ref().is_none_or(|b| score(&res.lyrics) > score(&b.lyrics)) {
                    best = Some(res);
                }
                if trusted || best.as_ref().is_some_and(|b| score(&b.lyrics) == SyncKind::Syllable.rank()) {
                    break;
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
    out.result = best;
    out
}

fn score(l: &Lyrics) -> u8 {
    if l.instrumental { 0 } else { l.kind.rank() }
}

fn trusted_local(registry: &Registry, c: &Candidate) -> bool {
    registry.prioritize_local && c.provider == super::local::ID && c.tier.is_some_and(|t| t <= MatchTier::Likely)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::future::BoxFuture;

    use super::*;
    use crate::lyrics::Credits;
    use crate::providers::{LyricsProvider, local};

    struct Mock {
        id: &'static str,
        kind: Option<SyncKind>,
        tier: MatchTier,
        calls: AtomicUsize,
    }

    impl LyricsProvider for Mock {
        fn id(&self) -> &'static str {
            self.id
        }

        fn name(&self) -> &'static str {
            self.id
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }

        fn lookup<'a>(&'a self, _q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Option<(Candidate, Lyrics)>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(self.kind.map(|kind| {
                    let c = Candidate {
                        provider: self.id.into(),
                        id: self.id.into(),
                        title: "Song".into(),
                        artists: vec!["Artist".into()],
                        album: None,
                        duration: None,
                        tier: Some(self.tier),
                        kind_hint: Some(kind),
                        note: None,
                    };
                    let l = Lyrics {
                        kind,
                        lines: vec![],
                        credits: Credits::default(),
                        language: None,
                        instrumental: false,
                    };
                    (c, l)
                }))
            })
        }

        fn search<'a>(&'a self, _q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Vec<Candidate>>> {
            Box::pin(async { Ok(vec![]) })
        }

        fn fetch<'a>(&'a self, _c: &'a Candidate) -> BoxFuture<'a, ProviderResult<Lyrics>> {
            Box::pin(async { Err(ProviderError::NotFound) })
        }
    }

    fn mock(id: &'static str, kind: Option<SyncKind>) -> Arc<Mock> {
        mock_tier(id, kind, MatchTier::Exact)
    }

    fn mock_tier(id: &'static str, kind: Option<SyncKind>, tier: MatchTier) -> Arc<Mock> {
        Arc::new(Mock {
            id,
            kind,
            tier,
            calls: AtomicUsize::new(0),
        })
    }

    async fn pick(providers: &[Arc<Mock>], prefer_synced: bool, prioritize_local: bool) -> Option<String> {
        let registry = Registry {
            providers: providers.iter().map(|p| p.clone() as Arc<dyn LyricsProvider>).collect(),
            prefer_synced,
            prioritize_local,
        };
        let cache = Cache::new(std::env::temp_dir().join("lyrminal-resolve-test"), 1, false);
        let q = TrackQuery {
            title: "Song".into(),
            artists: vec!["Artist".into()],
            ..Default::default()
        };
        resolve(&registry, &cache, &q, None, true)
            .await
            .result
            .map(|r| r.candidate.provider)
    }

    #[tokio::test]
    async fn better_sync_beats_priority() {
        let ps = [mock("a", Some(SyncKind::Line)), mock("b", Some(SyncKind::Line)), mock("c", Some(SyncKind::Syllable))];
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some("c"));
    }

    #[tokio::test]
    async fn priority_breaks_ties() {
        let ps = [mock("a", Some(SyncKind::Static)), mock("b", Some(SyncKind::Line)), mock("c", Some(SyncKind::Line))];
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some("b"));
    }

    #[tokio::test]
    async fn stops_once_word_sync_is_found() {
        let ps = [mock("a", Some(SyncKind::Syllable)), mock("b", Some(SyncKind::Syllable))];
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some("a"));
        assert_eq!(ps[1].calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn higher_provider_wins_a_tie_with_local() {
        let ps = [mock("a", Some(SyncKind::Line)), mock(local::ID, Some(SyncKind::Line))];
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some("a"));
    }

    #[tokio::test]
    async fn local_with_better_sync_beats_higher_provider() {
        let ps = [mock("a", Some(SyncKind::Line)), mock(local::ID, Some(SyncKind::Syllable))];
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some(local::ID));
    }

    #[tokio::test]
    async fn higher_provider_with_better_sync_beats_local() {
        let ps = [mock("a", Some(SyncKind::Syllable)), mock(local::ID, Some(SyncKind::Line))];
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some("a"));
    }

    #[tokio::test]
    async fn prioritized_local_beats_lower_providers() {
        let ps = [mock(local::ID, Some(SyncKind::Line)), mock("b", Some(SyncKind::Syllable))];
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some(local::ID));
        assert_eq!(ps[1].calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn local_competes_on_sync_when_not_prioritized() {
        let ps = [mock(local::ID, Some(SyncKind::Line)), mock("b", Some(SyncKind::Syllable))];
        assert_eq!(pick(&ps, true, false).await.as_deref(), Some("b"));
    }

    #[tokio::test]
    async fn loose_local_match_is_not_prioritized() {
        let ps = [
            mock_tier(local::ID, Some(SyncKind::Line), MatchTier::Version),
            mock("b", Some(SyncKind::Syllable)),
        ];
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some("b"));
    }

    #[tokio::test]
    async fn without_prefer_synced_first_match_wins() {
        let ps = [mock("a", None), mock("b", Some(SyncKind::Static)), mock("c", Some(SyncKind::Syllable))];
        assert_eq!(pick(&ps, false, true).await.as_deref(), Some("b"));
    }
}
