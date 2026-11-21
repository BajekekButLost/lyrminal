use serde::{Deserialize, Serialize};

use super::lrc::parse_timestamp;
use super::{Credits, LyricLine, Lyrics, SyncKind, VocalGroup, ass, json, lrc, sami, subtitles, ttml};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Lrc,
    Ttml,
    Srt,
    Vtt,
    Ass,
    Sbv,
    Sami,
    Json,
    Text,
}

impl Format {
    pub const ALL: [Format; 9] = [
        Format::Lrc,
        Format::Ttml,
        Format::Srt,
        Format::Vtt,
        Format::Ass,
        Format::Sbv,
        Format::Sami,
        Format::Json,
        Format::Text,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Format::Lrc => "LRC",
            Format::Ttml => "TTML",
            Format::Srt => "SRT",
            Format::Vtt => "WebVTT",
            Format::Ass => "ASS/SSA",
            Format::Sbv => "SBV",
            Format::Sami => "SAMI",
            Format::Json => "JSON",
            Format::Text => "plain text",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Lrc => "lrc",
            Format::Ttml => "ttml",
            Format::Srt => "srt",
            Format::Vtt => "vtt",
            Format::Ass => "ass",
            Format::Sbv => "sbv",
            Format::Sami => "smi",
            Format::Json => "json",
            Format::Text => "txt",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "lrc" | "elrc" => Format::Lrc,
            "ttml" | "dfxp" | "xml" => Format::Ttml,
            "srt" => Format::Srt,
            "vtt" | "webvtt" => Format::Vtt,
            "ass" | "ssa" => Format::Ass,
            "sbv" => Format::Sbv,
            "smi" | "sami" => Format::Sami,
            "json" => Format::Json,
            "txt" | "text" => Format::Text,
            _ => return None,
        })
    }

    pub fn from_name(name: &str) -> Option<Format> {
        let path = name.split(['?', '#']).next().unwrap_or(name);
        let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (_, ext) = file.rsplit_once('.')?;
        Format::from_extension(ext)
    }
}

pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|b| *b as char).collect(),
    }
}

fn is_lrc_line(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix('[') else { return false };
    rest.find(']').is_some_and(|c| parse_timestamp(&rest[..c]).is_some())
}

pub fn detect(text: &str) -> Format {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    let head: String = t.chars().take(8192).collect::<String>().to_ascii_lowercase();
    let looks_json = t.starts_with('{') || (t.starts_with('[') && t[1..].trim_start().starts_with(['{', '"', ']']));
    if looks_json && serde_json::from_str::<serde_json::Value>(t).is_ok() {
        return Format::Json;
    }
    if head.contains("<tt") {
        return Format::Ttml;
    }
    if head.contains("<sami") || head.contains("<sync") {
        return Format::Sami;
    }
    if t.starts_with("WEBVTT") {
        return Format::Vtt;
    }
    if head.contains("[script info]") || head.contains("[events]") || head.contains("\ndialogue:") {
        return Format::Ass;
    }
    if t.lines().any(|l| l.contains("-->")) {
        return Format::Srt;
    }
    if t.lines().any(|l| subtitles::sbv_timing(l).is_some()) {
        return Format::Sbv;
    }
    if t.lines().any(is_lrc_line) {
        return Format::Lrc;
    }
    Format::Text
}

pub fn plain(text: &str, credits: Credits) -> Option<Lyrics> {
    let lines: Vec<LyricLine> = text
        .trim()
        .lines()
        .map(|t| LyricLine {
            start: 0.0,
            end: 0.0,
            opposite: false,
            lead: VocalGroup::single(t.trim().to_string(), 0.0, 0.0),
            background: Vec::new(),
            background_only: false,
        })
        .collect();
    if lines.iter().all(|l| l.lead.syllables.iter().all(|s| s.text.is_empty())) {
        return None;
    }
    Some(Lyrics {
        kind: SyncKind::Static,
        lines,
        credits,
        language: None,
        instrumental: false,
    })
}

fn parse_as(text: &str, format: Format, credits: Credits, duration: Option<f64>) -> Option<Lyrics> {
    match format {
        Format::Lrc => lrc::parse(text, credits, duration),
        Format::Ttml => ttml::parse(text, credits),
        Format::Srt | Format::Vtt | Format::Sbv => subtitles::parse(text, credits),
        Format::Ass => ass::parse(text, credits),
        Format::Sami => sami::parse(text, credits),
        Format::Json => json::parse(text, credits, duration),
        Format::Text => plain(text, credits),
    }
}

pub fn parse(
    text: &str,
    format: Option<Format>,
    hint: Option<Format>,
    credits: Credits,
    duration: Option<f64>,
) -> Result<(Lyrics, Format), String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Err("the lyrics are empty".into());
    }
    let mut order: Vec<Format> = Vec::new();
    match format {
        Some(f) => order.push(f),
        None => {
            let detected = detect(text);
            order.extend(hint);
            if detected != Format::Text || hint.is_none_or(|h| h == Format::Text) {
                order.push(detected);
            }
            order.dedup();
        }
    }
    for f in &order {
        if let Some(mut lyrics) = parse_as(text, *f, credits.clone(), duration) {
            lyrics.sort();
            if lyrics.is_empty() {
                continue;
            }
            return Ok((lyrics, *f));
        }
    }
    let tried = order.iter().map(|f| f.label()).collect::<Vec<_>>().join(" or ");
    Err(format!("could not read the lyrics as {tried}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        assert_eq!(detect("[ti:x]\n[00:01.00]hi"), Format::Lrc);
        assert_eq!(detect("1\n00:00:01,000 --> 00:00:02,000\nhi"), Format::Srt);
        assert_eq!(detect("WEBVTT\n\n00:01.000 --> 00:02.000\nhi"), Format::Vtt);
        assert_eq!(detect("<?xml version=\"1.0\"?><tt xmlns=\"x\"></tt>"), Format::Ttml);
        assert_eq!(detect("[Script Info]\n[Events]\n"), Format::Ass);
        assert_eq!(detect("0:00:01.000,0:00:02.000\nhi"), Format::Sbv);
        assert_eq!(detect("<SAMI><BODY><SYNC Start=0>"), Format::Sami);
        assert_eq!(detect("{\"lyrics\":{}}"), Format::Json);
        assert_eq!(detect("just\nwords"), Format::Text);
        assert_eq!(Format::from_name("https://x.y/a/song.SRT?x=1"), Some(Format::Srt));
    }

    #[test]
    fn falls_back_to_detection_but_not_to_text() {
        let (l, f) = parse("[00:01.00]hi\n[00:02.00]yo", None, Some(Format::Srt), Credits::default(), None).unwrap();
        assert_eq!(f, Format::Lrc);
        assert_eq!(l.lines.len(), 2);
        assert!(parse("just words", None, Some(Format::Srt), Credits::default(), None).is_err());
        assert_eq!(parse("just words", None, None, Credits::default(), None).unwrap().1, Format::Text);
    }

    #[test]
    fn decodes_utf16_and_latin1() {
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("hé".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert_eq!(decode(&utf16), "hé");
        assert_eq!(decode(&[b'c', 0xE9]), "cé");
    }
}
