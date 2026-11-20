use futures::future::BoxFuture;
use serde::Deserialize;

use std::sync::Arc;
use std::time::Duration;

use super::{
    Candidate, LyricsProvider, ProviderError, ProviderResult, RateGate, TrackQuery, clean_title, gate_for, rate_tier,
    retry_after,
};
use crate::config::MatchTier;
use crate::lyrics::{Credits, LyricLine, Lyrics, Syllable, SyncKind, VocalGroup, lrc};

pub const ID: &str = "lrclib";
pub const NAME: &str = "LRCLIB";
const BASE: &str = "https://lrclib.net";

pub struct Lrclib {
    client: reqwest::Client,
    tolerance: f64,
    gate: Arc<RateGate>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    id: i64,
    track_name: String,
    artist_name: String,
    album_name: Option<String>,
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    has_word_sync: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    lyricsfile: Option<String>,
}

impl Record {
    fn kind_hint(&self) -> Option<SyncKind> {
        if self.has_word_sync {
            Some(SyncKind::Syllable)
        } else if self.synced_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            Some(SyncKind::Line)
        } else if self.plain_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            Some(SyncKind::Static)
        } else {
            None
        }
    }

    fn artists(&self) -> Vec<String> {
        vec![self.artist_name.clone()]
    }
}

#[derive(Debug, Deserialize)]
struct LyricsFile {
    #[serde(default)]
    metadata: Option<LyricsFileMeta>,
    #[serde(default)]
    lines: Vec<LfLine>,
}

#[derive(Debug, Deserialize)]
struct LyricsFileMeta {
    language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LfLine {
    #[serde(default)]
    text: String,
    start_ms: Option<f64>,
    end_ms: Option<f64>,
    #[serde(default)]
    words: Vec<LfWord>,
}

#[derive(Debug, Deserialize)]
struct LfWord {
    #[serde(default)]
    text: String,
    start_ms: Option<f64>,
    end_ms: Option<f64>,
}

impl Lrclib {
    pub fn new(client: reqwest::Client, tolerance: f64) -> Self {
        Self {
            client,
            tolerance,
            gate: gate_for(ID, Duration::from_millis(350)),
        }
    }

    async fn request<T: serde::de::DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> ProviderResult<T> {
        self.gate.acquire().await?;
        let result = self.send(path, query).await;
        self.gate.observe(result).await
    }

    async fn send<T: serde::de::DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> ProviderResult<T> {
        let resp = self.client.get(format!("{BASE}{path}")).query(query).send().await?;
        let status = resp.status().as_u16();
        match status {
            200 => resp.json::<T>().await.map_err(Into::into),
            404 => Err(ProviderError::NotFound),
            429 => Err(ProviderError::RateLimited(retry_after(&resp))),
            503 => Err(ProviderError::Unavailable("server overloaded".into(), retry_after(&resp))),
            _ => Err(ProviderError::Http(status, resp.text().await.unwrap_or_default())),
        }
    }

    fn signature_query(&self, q: &TrackQuery, title: &str) -> Vec<(&'static str, String)> {
        let mut query = vec![("track_name", title.to_string()), ("artist_name", q.artist_line())];
        if let Some(album) = q.album.as_ref().filter(|a| !a.is_empty()) {
            query.push(("album_name", album.clone()));
        }
        if let Some(d) = q.duration.filter(|d| (1.0..=3600.0).contains(d)) {
            query.push(("duration", format!("{}", d.round() as i64)));
        }
        query
    }

    async fn search_records(&self, q: &TrackQuery) -> ProviderResult<Vec<Record>> {
        let mut query = vec![("track_name", clean_title(&q.title))];
        if !q.artists.is_empty() {
            query.push(("artist_name", q.artists[0].clone()));
        }
        let mut records: Vec<Record> = self.request("/api/search", &query).await?;
        if records.is_empty() {
            let free = format!("{} {}", clean_title(&q.title), q.artists.first().cloned().unwrap_or_default());
            records = self.request("/api/search", &[("q", free)]).await?;
        }
        Ok(records)
    }

    fn candidate(&self, q: &TrackQuery, r: &Record) -> Candidate {
        Candidate {
            provider: ID.into(),
            id: r.id.to_string(),
            title: r.track_name.clone(),
            artists: r.artists(),
            album: r.album_name.clone(),
            duration: r.duration,
            tier: Some(rate_tier(q, &r.track_name, &r.artists(), r.duration, self.tolerance)),
            kind_hint: r.kind_hint(),
            note: r.instrumental.then(|| "instrumental".to_string()),
        }
    }
}

fn credits() -> Credits {
    Credits {
        provider: NAME.into(),
        provider_url: Some("https://lrclib.net".into()),
        source: None,
        songwriters: Vec::new(),
        contributors: Vec::new(),
    }
}

fn convert(r: &Record) -> ProviderResult<Lyrics> {
    if r.instrumental {
        return Ok(Lyrics {
            kind: SyncKind::Static,
            lines: Vec::new(),
            credits: credits(),
            language: None,
            instrumental: true,
        });
    }
    if r.has_word_sync
        && let Some(doc) = &r.lyricsfile
        && let Some(l) = parse_lyricsfile(doc, r.duration)
    {
        return Ok(l);
    }
    if let Some(synced) = r.synced_lyrics.as_deref().filter(|s| !s.trim().is_empty())
        && let Some(l) = lrc::parse(synced, credits(), r.duration)
    {
        return Ok(l);
    }
    if let Some(plain) = r.plain_lyrics.as_deref().filter(|s| !s.trim().is_empty()) {
        let lines = plain
            .lines()
            .map(|t| LyricLine {
                start: 0.0,
                end: 0.0,
                opposite: false,
                lead: VocalGroup::single(t.trim().to_string(), 0.0, 0.0),
                background: Vec::new(),
            })
            .collect();
        return Ok(Lyrics {
            kind: SyncKind::Static,
            lines,
            credits: credits(),
            language: None,
            instrumental: false,
        });
    }
    Err(ProviderError::NotFound)
}

fn parse_lyricsfile(doc: &str, duration: Option<f64>) -> Option<Lyrics> {
    let file: LyricsFile = serde_yaml_ng::from_str(doc).ok()?;
    let n = file.lines.len();
    let mut lines = Vec::new();
    for (i, l) in file.lines.iter().enumerate() {
        if l.text.trim().is_empty() && l.words.is_empty() {
            continue;
        }
        let start = l.start_ms? / 1000.0;
        let next_start = file.lines[i + 1..n]
            .iter()
            .find_map(|n| n.start_ms)
            .map(|s| s / 1000.0);
        let end = l
            .end_ms
            .map(|e| e / 1000.0)
            .or(next_start)
            .or(duration)
            .unwrap_or(start + 5.0)
            .max(start);
        let mut syllables = Vec::new();
        for (wi, w) in l.words.iter().enumerate() {
            let trimmed = w.text.trim();
            if trimmed.is_empty() {
                if let Some(last) = syllables.last_mut() {
                    let last: &mut Syllable = last;
                    last.part_of_word = false;
                }
                continue;
            }
            let ws = w.start_ms.map(|s| s / 1000.0).unwrap_or(start);
            let we = w
                .end_ms
                .map(|e| e / 1000.0)
                .or_else(|| l.words[wi + 1..].iter().find_map(|n| n.start_ms).map(|s| s / 1000.0))
                .unwrap_or(end)
                .max(ws);
            syllables.push(Syllable {
                text: trimmed.to_string(),
                start: ws,
                end: we,
                part_of_word: !w.text.ends_with(char::is_whitespace) && wi + 1 < l.words.len(),
                roman: None,
                roman_part_of_word: false,
            });
        }
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
        lines.push(LyricLine {
            start,
            end,
            opposite: false,
            lead,
            background: Vec::new(),
        });
    }
    if lines.is_empty() {
        return None;
    }
    Some(Lyrics {
        kind: SyncKind::Syllable,
        lines,
        credits: credits(),
        language: file.metadata.and_then(|m| m.language),
        instrumental: false,
    })
}

impl LyricsProvider for Lrclib {
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
            if !q.is_searchable() || q.artists.is_empty() {
                return Ok(None);
            }
            let mut titles = vec![q.title.clone()];
            let cleaned = clean_title(&q.title);
            if cleaned != q.title {
                titles.push(cleaned);
            }
            for title in &titles {
                match self.request::<Record>("/api/get", &self.signature_query(q, title)).await {
                    Ok(r) => {
                        let lyrics = convert(&r)?;
                        return Ok(Some((self.candidate(q, &r), lyrics)));
                    }
                    Err(ProviderError::NotFound) => {}
                    Err(e) => return Err(e),
                }
            }
            let records = self.search_records(q).await?;
            let mut best: Option<(MatchTier, u8, &Record)> = None;
            for r in &records {
                let tier = rate_tier(q, &r.track_name, &r.artists(), r.duration, self.tolerance);
                if tier > MatchTier::Exact {
                    continue;
                }
                let rank = r.kind_hint().map(|k| k.rank()).unwrap_or(0);
                if best.as_ref().is_none_or(|(_, br, _)| rank > *br) {
                    best = Some((tier, rank, r));
                }
            }
            match best {
                Some((_, _, r)) => Ok(Some((self.candidate(q, r), convert(r)?))),
                None => Ok(None),
            }
        })
    }

    fn search<'a>(&'a self, q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Vec<Candidate>>> {
        Box::pin(async move {
            if !q.is_searchable() {
                return Ok(Vec::new());
            }
            let records = self.search_records(q).await?;
            let mut out: Vec<Candidate> = records.iter().map(|r| self.candidate(q, r)).collect();
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
            let r: Record = self.request(&format!("/api/get/{}", c.id.trim()), &[]).await?;
            convert(&r)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lyricsfile_words() {
        let doc = "version: '1.0'\nmetadata:\n  language: en\nlines:\n- text: ab cd\n  start_ms: 1000\n  end_ms: 3000\n  words:\n  - text: 'ab '\n    start_ms: 1000\n  - text: cd\n    start_ms: 2000\n";
        let l = parse_lyricsfile(doc, None).unwrap();
        assert_eq!(l.kind, SyncKind::Syllable);
        let s = &l.lines[0].lead.syllables;
        assert_eq!(s.len(), 2);
        assert!(!s[0].part_of_word);
        assert!((s[0].end - 2.0).abs() < 1e-9);
        assert!((s[1].end - 3.0).abs() < 1e-9);
        assert_eq!(l.language.as_deref(), Some("en"));
    }
}
