use serde_json::Value;

use super::cue::{self, Cue};
use super::{Credits, Lyrics, formats, lrc, ttml};

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn find_ttml(v: &Value) -> Option<&str> {
    match v {
        Value::Object(map) => {
            for key in ["ttml", "ttmlLocalizations"] {
                if let Some(Value::String(s)) = map.get(key)
                    && s.contains("<tt")
                {
                    return Some(s);
                }
            }
            map.values().find_map(find_ttml)
        }
        Value::Array(items) => items.iter().find_map(find_ttml),
        _ => None,
    }
}

fn spotify(v: &Value, credits: Credits) -> Option<Lyrics> {
    let obj = v.get("lyrics").unwrap_or(v);
    let lines = obj.get("lines")?.as_array()?;
    let first = lines.first()?;
    first.get("words")?;
    let unsynced = obj.get("syncType").and_then(Value::as_str) == Some("UNSYNCED")
        || lines.iter().all(|l| l.get("startTimeMs").and_then(number).unwrap_or(0.0) == 0.0);
    let texts: Vec<(f64, f64, String)> = lines
        .iter()
        .map(|l| {
            (
                l.get("startTimeMs").and_then(number).unwrap_or(0.0) / 1000.0,
                l.get("endTimeMs").and_then(number).unwrap_or(0.0) / 1000.0,
                l.get("words").and_then(Value::as_str).unwrap_or("").trim().to_string(),
            )
        })
        .collect();
    if unsynced {
        let joined = texts.iter().map(|(_, _, t)| t.as_str()).collect::<Vec<_>>().join("\n");
        return formats::plain(&joined, credits);
    }
    let cues = texts
        .iter()
        .enumerate()
        .filter(|(_, (_, _, t))| !t.is_empty() && t != "♪")
        .map(|(i, (start, end, text))| {
            let next = texts.get(i + 1).map(|(s, _, _)| *s);
            let end = if *end > *start { *end } else { next.unwrap_or(start + 5.0) };
            Cue {
                start: *start,
                end,
                text: text.clone(),
                segments: Vec::new(),
                voice: None,
            }
        })
        .collect();
    cue::build(cues, credits)
}

fn lrclib(v: &Value, credits: Credits, duration: Option<f64>) -> Option<Lyrics> {
    let record = match v {
        Value::Array(items) => items.iter().find(|i| i.get("syncedLyrics").or_else(|| i.get("plainLyrics")).is_some())?,
        _ => v,
    };
    let duration = record.get("duration").and_then(number).or(duration);
    if let Some(s) = record.get("syncedLyrics").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
        && let Some(l) = lrc::parse(s, credits.clone(), duration)
    {
        return Some(l);
    }
    let plain = record.get("plainLyrics").and_then(Value::as_str)?;
    formats::plain(plain, credits)
}

pub fn parse(input: &str, credits: Credits, duration: Option<f64>) -> Option<Lyrics> {
    let v: Value = serde_json::from_str(input.trim_start_matches('\u{feff}')).ok()?;
    if let Some(doc) = find_ttml(&v) {
        return ttml::parse(doc, credits);
    }
    spotify(&v, credits.clone()).or_else(|| lrclib(&v, credits, duration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::SyncKind;

    #[test]
    fn parses_spotify_json() {
        let src = r#"{"lyrics":{"syncType":"LINE_SYNCED","lines":[{"startTimeMs":"1000","words":"One","endTimeMs":"0"},{"startTimeMs":"2500","words":"♪","endTimeMs":"0"},{"startTimeMs":"4000","words":"Two","endTimeMs":"0"}]}}"#;
        let l = parse(src, Credits::default(), None).unwrap();
        assert_eq!(l.kind, SyncKind::Line);
        assert_eq!(l.lines.len(), 2);
        assert!((l.lines[0].end - 2.5).abs() < 1e-9);
    }

    #[test]
    fn parses_lrclib_and_wrapped_ttml() {
        let src = r#"{"id":1,"syncedLyrics":"[00:01.00]a\n[00:02.00]b","plainLyrics":"a\nb"}"#;
        assert_eq!(parse(src, Credits::default(), None).unwrap().kind, SyncKind::Line);
        let src = r#"{"data":[{"attributes":{"ttml":"<tt xmlns=\"http://www.w3.org/ns/ttml\"><body><div><p begin=\"1.0\" end=\"2.0\">hi</p></div></body></tt>"}}]}"#;
        assert_eq!(parse(src, Credits::default(), None).unwrap().lines.len(), 1);
    }
}
