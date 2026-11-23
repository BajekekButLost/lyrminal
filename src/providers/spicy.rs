use futures::future::{BoxFuture, join_all};
use serde::Deserialize;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Candidate, LyricsProvider, ProviderError, ProviderResult, RateGate, TrackQuery, gate_for, retry_after};
use crate::config::MatchTier;
use crate::lyrics::{Contributor, Credits, LyricLine, Lyrics, Syllable, SyncKind, VocalGroup};

pub const ID: &str = "spicy-lyrics";
pub const NAME: &str = "Spicy Lyrics";
const BASE: &str = "https://api.spicylyrics.org";

pub struct SpicyLyrics {
    client: reqwest::Client,
    key: String,
    min_tier: MatchTier,
    check_results: Option<usize>,
    checked: Mutex<HashMap<String, Lyrics>>,
    gate: Arc<RateGate>,
}

impl SpicyLyrics {
    pub fn new(client: reqwest::Client, key: String, min_tier: MatchTier, check_results: Option<usize>) -> Self {
        Self {
            client,
            key,
            min_tier,
            check_results,
            checked: Mutex::new(HashMap::new()),
            gate: gate_for(ID, Duration::from_millis(250)),
        }
    }

    fn checked(&self) -> std::sync::MutexGuard<'_, HashMap<String, Lyrics>> {
        self.checked.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn get_json(&self, url: &str, query: &[(&str, String)]) -> ProviderResult<serde_json::Value> {
        self.gate.acquire().await?;
        let result = self.send(&self.key, url, query).await;
        self.gate.observe(result).await
    }

    async fn send(&self, key: &str, url: &str, query: &[(&str, String)]) -> ProviderResult<serde_json::Value> {
        let resp = self
            .client
            .get(url)
            .bearer_auth(key)
            .query(query)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status == 200 {
            return resp.json::<serde_json::Value>().await.map_err(Into::into);
        }
        let retry = retry_after(&resp);
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let code = body["Body"]["error"].as_str().unwrap_or("").to_string();
        let message = body["Body"]["message"].as_str().unwrap_or("").to_string();
        Err(match status {
            404 => ProviderError::NotFound,
            429 => ProviderError::RateLimited(retry),
            503 => ProviderError::Unavailable(if message.is_empty() { code } else { message }, retry),
            401 | 403 => ProviderError::NotConfigured(if message.is_empty() { code } else { message }),
            _ => ProviderError::Http(status, if message.is_empty() { code } else { message }),
        })
    }

    async fn lyrics_by_id(&self, id: &str) -> ProviderResult<Lyrics> {
        let url = format!("{BASE}/v1/lyrics/{}", urlencode_path(id));
        let json = self.get_json(&url, &[]).await?;
        let body = json
            .get("Body")
            .ok_or_else(|| ProviderError::Parse("missing Body".into()))?;
        let parsed: Body =
            serde_json::from_value(body.clone()).map_err(|e| ProviderError::Parse(e.to_string()))?;
        convert(parsed)
    }

    async fn search_results(&self, q: &TrackQuery) -> ProviderResult<Vec<SearchResult>> {
        let mut query: Vec<(&str, String)> = vec![("title", q.title.clone()), ("limit", "10".into())];
        if !q.artists.is_empty() {
            query.push(("artist", q.artist_line()));
        }
        if let Some(album) = q.album.as_ref().filter(|a| !a.is_empty()) {
            query.push(("album", album.clone()));
        }
        if let Some(d) = q.duration {
            query.push(("duration", format!("{d:.3}")));
        }
        let json = self.get_json(&format!("{BASE}/v1/tracks/search"), &query).await?;
        let results = json["Body"]["results"].clone();
        serde_json::from_value(results).map_err(|e| ProviderError::Parse(e.to_string()))
    }

    async fn keep_available(&self, candidates: Vec<Candidate>) -> ProviderResult<Vec<Candidate>> {
        let probes = join_all(candidates.iter().map(|c| self.lyrics_by_id(&c.id))).await;
        let mut kept = Vec::new();
        let mut found = HashMap::new();
        let mut failure = None;
        for (mut c, probe) in candidates.into_iter().zip(probes) {
            match probe {
                Ok(lyrics) => {
                    c.kind_hint = Some(lyrics.kind);
                    found.insert(c.id.clone(), lyrics);
                    kept.push(c);
                }
                Err(ProviderError::NotFound) => {}
                Err(e) => {
                    failure.get_or_insert(e);
                }
            }
        }
        *self.checked() = found;
        match failure {
            Some(e) if kept.is_empty() => Err(e),
            _ => Ok(kept),
        }
    }

    fn candidate(&self, r: &SearchResult, id: &str, album: Option<String>, duration: f64, note: Option<String>) -> Candidate {
        Candidate {
            provider: ID.into(),
            id: id.to_string(),
            title: r.title.clone(),
            artists: r.artists.clone(),
            album,
            duration: Some(duration),
            tier: parse_tier(&r.tier),
            kind_hint: None,
            note,
        }
    }
}

fn urlencode_path(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

fn parse_tier(t: &str) -> Option<MatchTier> {
    Some(match t {
        "exact" => MatchTier::Exact,
        "likely" => MatchTier::Likely,
        "version" => MatchTier::Version,
        "other_artist" => MatchTier::OtherArtist,
        "related" => MatchTier::Related,
        _ => return None,
    })
}

impl LyricsProvider for SpicyLyrics {
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
            let direct = q.spotify_id.clone().or_else(|| q.isrc.clone());
            if let Some(id) = direct {
                return match self.lyrics_by_id(&id).await {
                    Ok(lyrics) => Ok(Some((
                        Candidate {
                            provider: ID.into(),
                            id,
                            title: q.title.clone(),
                            artists: q.artists.clone(),
                            album: q.album.clone(),
                            duration: q.duration,
                            tier: Some(MatchTier::Exact),
                            kind_hint: Some(lyrics.kind),
                            note: Some("matched by track id".into()),
                        },
                        lyrics,
                    ))),
                    Err(ProviderError::NotFound) => Ok(None),
                    Err(e) => Err(e),
                };
            }
            if !q.is_searchable() {
                return Ok(None);
            }
            let results = self.search_results(q).await?;
            let Some(best) = results.first() else { return Ok(None) };
            let tier = parse_tier(&best.tier).unwrap_or(MatchTier::Related);
            if tier > self.min_tier {
                return Ok(None);
            }
            match self.lyrics_by_id(&best.id).await {
                Ok(lyrics) => {
                    let mut c = self.candidate(best, &best.id, best.album.clone(), best.duration, None);
                    c.kind_hint = Some(lyrics.kind);
                    Ok(Some((c, lyrics)))
                }
                Err(ProviderError::NotFound) => Ok(None),
                Err(e) => Err(e),
            }
        })
    }

    fn search<'a>(&'a self, q: &'a TrackQuery) -> BoxFuture<'a, ProviderResult<Vec<Candidate>>> {
        Box::pin(async move {
            let mut out = Vec::new();
            if let Some(id) = &q.spotify_id {
                out.push(Candidate {
                    provider: ID.into(),
                    id: id.clone(),
                    title: q.title.clone(),
                    artists: q.artists.clone(),
                    album: q.album.clone(),
                    duration: q.duration,
                    tier: Some(MatchTier::Exact),
                    kind_hint: None,
                    note: Some("playing track".into()),
                });
            }
            if q.is_searchable() {
                for r in self.search_results(q).await? {
                    if !out.iter().any(|c| c.id == r.id) {
                        out.push(self.candidate(&r, &r.id, r.album.clone(), r.duration, None));
                    }
                    for v in &r.variants {
                        if !out.iter().any(|c| c.id == v.id) {
                            out.push(self.candidate(
                                &r,
                                &v.id,
                                v.album.clone(),
                                v.duration,
                                Some("other release".into()),
                            ));
                        }
                    }
                }
            }
            let Some(limit) = self.check_results else {
                return Ok(out);
            };
            out.truncate(limit.max(1));
            self.keep_available(out).await
        })
    }

    fn fetch<'a>(&'a self, c: &'a Candidate) -> BoxFuture<'a, ProviderResult<Lyrics>> {
        Box::pin(async move {
            let checked = self.checked().remove(&c.id);
            match checked {
                Some(lyrics) => Ok(lyrics),
                None => self.lyrics_by_id(&c.id).await,
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    id: String,
    title: String,
    #[serde(default)]
    artists: Vec<String>,
    album: Option<String>,
    duration: f64,
    tier: String,
    #[serde(default)]
    variants: Vec<Variant>,
}

#[derive(Debug, Deserialize)]
struct Variant {
    id: String,
    album: Option<String>,
    duration: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Body {
    #[serde(rename = "Type")]
    kind: String,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    lines: Option<Vec<StaticLine>>,
    #[serde(default)]
    song_writers: Vec<String>,
    #[serde(default)]
    upload_attribution: Option<Attribution>,
    #[serde(rename = "source", default)]
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Attribution {
    uploader: Option<Person>,
    maker: Option<Person>,
}

#[derive(Debug, Deserialize)]
struct Person {
    username: String,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct StaticLine {
    text: String,
    transliterated_text: Option<String>,
    translated_text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LineLine {
    #[serde(rename = "Type", default)]
    kind: Option<String>,
    text: String,
    start_time: f64,
    end_time: f64,
    #[serde(default)]
    opposite_aligned: bool,
    transliterated_text: Option<String>,
    translated_text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SylLine {
    #[serde(rename = "Type", default)]
    kind: Option<String>,
    #[serde(default)]
    opposite_aligned: bool,
    #[serde(default)]
    lead: Group,
    #[serde(default)]
    background: Vec<Group>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Group {
    #[serde(default)]
    syllables: Vec<Syl>,
    start_time: Option<f64>,
    end_time: Option<f64>,
    transliterated_text: Option<String>,
    translated_text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Syl {
    text: String,
    start_time: Option<f64>,
    end_time: Option<f64>,
    #[serde(default)]
    is_part_of_word: bool,
    transliterated_text: Option<String>,
    #[serde(default)]
    transliterated_is_part_of_word: bool,
}

fn source_label(source: Option<&str>) -> String {
    match source {
        Some("spicy_lyrics") => "Community sync".into(),
        Some("apple_music") => "Apple Music".into(),
        Some("spotify") => "Spotify".into(),
        _ => "Unknown source".into(),
    }
}

fn source_tag(source: Option<&str>) -> Option<String> {
    match source {
        Some("spicy_lyrics") => Some("Community".into()),
        Some("apple_music") => Some("Apple Music".into()),
        Some("spotify") => Some("Spotify".into()),
        _ => None,
    }
}

fn convert_group(g: Group) -> VocalGroup {
    let syllables: Vec<Syllable> = g
        .syllables
        .into_iter()
        .map(|s| {
            let start = s.start_time.unwrap_or(0.0);
            Syllable {
                text: s.text,
                start,
                end: s.end_time.unwrap_or(start),
                part_of_word: s.is_part_of_word,
                roman: s.transliterated_text,
                roman_part_of_word: s.transliterated_is_part_of_word,
            }
        })
        .collect();
    let start = g
        .start_time
        .or_else(|| syllables.first().map(|s| s.start))
        .unwrap_or(0.0);
    let end = g
        .end_time
        .or_else(|| syllables.last().map(|s| s.end))
        .unwrap_or(start);
    VocalGroup {
        start,
        end,
        syllables,
        roman: g.transliterated_text,
        translation: g.translated_text,
    }
}

fn convert(body: Body) -> ProviderResult<Lyrics> {
    let mut contributors = Vec::new();
    if body.source.as_deref() == Some("spicy_lyrics")
        && let Some(attr) = body.upload_attribution
    {
        if let Some(m) = attr.maker {
            contributors.push(Contributor {
                role: "Synced by".into(),
                name: m.username,
                url: m.url,
            });
        }
        if let Some(u) = attr.uploader {
            contributors.push(Contributor {
                role: "Uploaded by".into(),
                name: u.username,
                url: u.url,
            });
        }
    }
    let credits = Credits {
        provider: NAME.into(),
        provider_url: Some("https://spicylyrics.org".into()),
        provider_credit: None,
        source: Some(source_label(body.source.as_deref())),
        source_tag: source_tag(body.source.as_deref()),
        songwriters: body.song_writers,
        contributors,
    };
    let parse_err = |e: serde_json::Error| ProviderError::Parse(e.to_string());
    let (kind, lines) = match body.kind.as_str() {
        "Syllable" => {
            let raw: Vec<SylLine> =
                serde_json::from_value(body.content.unwrap_or_default()).map_err(parse_err)?;
            let lines = raw
                .into_iter()
                .filter(|l| l.kind.as_deref().is_none_or(|k| k == "Vocal"))
                .map(|l| {
                    let lead = convert_group(l.lead);
                    let background: Vec<VocalGroup> = l.background.into_iter().map(convert_group).collect();
                    LyricLine {
                        start: lead.start,
                        end: lead.end,
                        opposite: l.opposite_aligned,
                        lead,
                        background,
                        background_only: false,
                    }
                })
                .collect();
            (SyncKind::Syllable, lines)
        }
        "Line" => {
            let raw: Vec<LineLine> =
                serde_json::from_value(body.content.unwrap_or_default()).map_err(parse_err)?;
            let lines = raw
                .into_iter()
                .filter(|l| l.kind.as_deref().is_none_or(|k| k == "Vocal"))
                .map(|l| {
                    let mut lead = VocalGroup::single(l.text, l.start_time, l.end_time);
                    lead.roman = l.transliterated_text;
                    lead.translation = l.translated_text;
                    LyricLine {
                        start: l.start_time,
                        end: l.end_time,
                        opposite: l.opposite_aligned,
                        lead,
                        background: Vec::new(),
                        background_only: false,
                    }
                })
                .collect();
            (SyncKind::Line, lines)
        }
        "Static" => {
            let lines = body
                .lines
                .unwrap_or_default()
                .into_iter()
                .map(|l| {
                    let mut lead = VocalGroup::single(l.text, 0.0, 0.0);
                    lead.roman = l.transliterated_text;
                    lead.translation = l.translated_text;
                    LyricLine {
                        start: 0.0,
                        end: 0.0,
                        opposite: false,
                        lead,
                        background: Vec::new(),
                        background_only: false,
                    }
                })
                .collect();
            (SyncKind::Static, lines)
        }
        other => return Err(ProviderError::Parse(format!("unknown lyrics type {other}"))),
    };
    let mut lyrics = Lyrics {
        kind,
        lines,
        credits,
        language: None,
        instrumental: false,
    };
    lyrics.sort();
    if lyrics.is_empty() {
        return Err(ProviderError::NotFound);
    }
    Ok(lyrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_reuses_checked_lyrics_once() {
        let p = SpicyLyrics::new(reqwest::Client::new(), String::new(), MatchTier::Exact, Some(5));
        let body: Body = serde_json::from_value(serde_json::json!({"Type": "Static", "Lines": [{"Text": "a"}], "source": "spotify"})).unwrap();
        p.checked().insert("x".into(), convert(body).unwrap());
        let c = Candidate {
            provider: ID.into(),
            id: "x".into(),
            title: "t".into(),
            artists: vec![],
            album: None,
            duration: None,
            tier: None,
            kind_hint: None,
            note: None,
        };
        let l = p.fetch(&c).await.unwrap();
        assert_eq!(l.lines[0].lead.text(), "a");
        assert!(p.checked().is_empty());
    }

    #[test]
    fn converts_syllable_body() {
        let json = serde_json::json!({
            "Type": "Syllable",
            "SongWriters": ["A"],
            "Content": [{
                "Type": "Vocal",
                "OppositeAligned": true,
                "Lead": {
                    "Syllables": [
                        {"Text": "he", "StartTime": 1.0, "EndTime": 1.2, "IsPartOfWord": true},
                        {"Text": "llo", "StartTime": 1.2, "EndTime": 1.5, "IsPartOfWord": false}
                    ],
                    "StartTime": 1.0, "EndTime": 1.5
                },
                "Background": [{"Syllables": [{"Text": "oh", "StartTime": 1.6, "EndTime": 2.0}]}]
            }],
            "id": "x", "ids": {"spotify": "x"},
            "UploadAttribution": {"Uploader": {"id": "1", "username": "u", "url": "https://x/u"}},
            "source": "spicy_lyrics"
        });
        let body: Body = serde_json::from_value(json).unwrap();
        let l = convert(body).unwrap();
        assert_eq!(l.kind, SyncKind::Syllable);
        assert!(l.lines[0].opposite);
        assert_eq!(l.lines[0].lead.text(), "hello");
        assert_eq!(l.lines[0].background.len(), 1);
        assert_eq!(l.credits.contributors.len(), 1);
        assert_eq!(l.credits.contributors[0].role, "Uploaded by");
    }

    #[test]
    fn keeps_background_only_and_early_background_lines() {
        let json = serde_json::json!({
            "Type": "Syllable",
            "Content": [
                {
                    "Type": "Vocal",
                    "OppositeAligned": true,
                    "Lead": {"Syllables": []},
                    "Background": [{"Syllables": [{"Text": "(ooh)", "StartTime": 0.5, "EndTime": 1.5}]}]
                },
                {
                    "Type": "Vocal",
                    "Lead": {"Syllables": [{"Text": "go", "StartTime": 3.0, "EndTime": 3.5}], "StartTime": 3.0, "EndTime": 3.5},
                    "Background": [{"Syllables": [{"Text": "(hey)", "StartTime": 2.2, "EndTime": 2.8}]}]
                }
            ],
            "id": "x", "ids": {"spotify": "x"}, "source": "spotify"
        });
        let body: Body = serde_json::from_value(json).unwrap();
        let l = convert(body).unwrap();
        assert_eq!(l.lines.len(), 2);
        let first = &l.lines[0];
        assert!(first.background_only && first.opposite);
        assert_eq!(first.lead.text(), "(ooh)");
        assert!((first.start - 0.5).abs() < 1e-9);
        assert!((l.lines[1].start - 2.2).abs() < 1e-9);
        assert!(!l.lines[1].background_only);
    }

    #[test]
    fn omits_attribution_for_commercial_sources() {
        let json = serde_json::json!({
            "Type": "Static", "Lines": [{"Text": "a"}], "id": "x", "ids": {"spotify": "x"},
            "source": "spotify"
        });
        let body: Body = serde_json::from_value(json).unwrap();
        let l = convert(body).unwrap();
        assert!(l.credits.contributors.is_empty());
        assert_eq!(l.credits.source.as_deref(), Some("Spotify"));
        assert_eq!(l.credits.source_tag.as_deref(), Some("Spotify"));
    }

    #[test]
    fn labels_unknown_source() {
        let json = serde_json::json!({"Type": "Static", "Lines": [{"Text": "a"}], "id": "x", "ids": {"spotify": "x"}, "source": "unknown"});
        let body: Body = serde_json::from_value(json).unwrap();
        let l = convert(body).unwrap();
        assert_eq!(l.credits.source.as_deref(), Some("Unknown source"));
        assert_eq!(l.credits.source_tag, None);
    }
}
