use futures::StreamExt;
use futures::stream::FuturesOrdered;

use super::cache::Cache;
use super::{Candidate, ProviderError, ProviderResult, RateLimit, Registry, TrackQuery};
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
    pub rate_limits: Vec<RateLimit>,
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
            Err(e) => {
                let name = super::provider_name(&c.provider);
                out.rate_limits.extend(e.rate_limit(name));
                out.attempts.push(Attempt {
                    provider: name.to_string(),
                    outcome: format!("chosen sync failed: {e}"),
                });
            }
        }
    }
    let key = q.key();
    let mut lookups: FuturesOrdered<_> = registry
        .providers
        .iter()
        .map(|p| {
            let key = &key;
            async move {
                if let Err(reason) = p.ready() {
                    return (p.name(), Err(reason));
                }
                let cacheable = super::cacheable(p.id());
                let cached = if bypass_cache || !cacheable { None } else { cache.get_lookup(p.id(), key) };
                let looked = match cached {
                    Some(hit) => Ok(hit),
                    None => {
                        let r = p.lookup(q).await;
                        if cacheable && let Ok(v) = &r {
                            cache.put_lookup(p.id(), key, v);
                        }
                        r
                    }
                };
                (p.name(), Ok(looked))
            }
        })
        .collect();
    let mut best: Option<Resolution> = None;
    while let Some((name, looked)) = lookups.next().await {
        let looked = match looked {
            Ok(looked) => looked,
            Err(reason) => {
                out.attempts.push(Attempt {
                    provider: name.into(),
                    outcome: format!("skipped: {reason}"),
                });
                continue;
            }
        };
        match looked {
            Ok(Some((candidate, lyrics))) => {
                out.attempts.push(Attempt {
                    provider: name.into(),
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
                provider: name.into(),
                outcome: "no match".into(),
            }),
            Err(e) => {
                out.rate_limits.extend(e.rate_limit(name));
                out.attempts.push(Attempt {
                    provider: name.into(),
                    outcome: e.to_string(),
                });
            }
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
    use std::time::{Duration, Instant};

    use futures::future::BoxFuture;

    use super::*;
    use crate::lyrics::Credits;
    use crate::providers::{LyricsProvider, local};

    struct Mock {
        id: &'static str,
        kind: Option<SyncKind>,
        tier: MatchTier,
        delay: Duration,
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
            Box::pin(async move {
                tokio::time::sleep(self.delay).await;
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
        mock_with(id, kind, MatchTier::Exact, Duration::ZERO)
    }

    fn mock_tier(id: &'static str, kind: Option<SyncKind>, tier: MatchTier) -> Arc<Mock> {
        mock_with(id, kind, tier, Duration::ZERO)
    }

    fn mock_slow(id: &'static str, kind: Option<SyncKind>, delay_ms: u64) -> Arc<Mock> {
        mock_with(id, kind, MatchTier::Exact, Duration::from_millis(delay_ms))
    }

    fn mock_with(id: &'static str, kind: Option<SyncKind>, tier: MatchTier, delay: Duration) -> Arc<Mock> {
        Arc::new(Mock { id, kind, tier, delay })
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
    async fn asks_all_providers_at_once() {
        let ps = [
            mock_slow("a", Some(SyncKind::Line), 200),
            mock_slow("b", Some(SyncKind::Line), 200),
            mock_slow("c", Some(SyncKind::Syllable), 200),
        ];
        let started = Instant::now();
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some("c"));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn stops_waiting_once_word_sync_is_found() {
        let ps = [mock("a", Some(SyncKind::Syllable)), mock_slow("b", Some(SyncKind::Syllable), 5000)];
        let started = Instant::now();
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some("a"));
        assert!(started.elapsed() < Duration::from_secs(1));
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
        let ps = [mock(local::ID, Some(SyncKind::Line)), mock_slow("b", Some(SyncKind::Syllable), 5000)];
        let started = Instant::now();
        assert_eq!(pick(&ps, true, true).await.as_deref(), Some(local::ID));
        assert!(started.elapsed() < Duration::from_secs(1));
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
