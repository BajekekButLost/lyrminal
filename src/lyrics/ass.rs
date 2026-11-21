use super::cue::{self, Cue};
use super::lrc::parse_timestamp;
use super::{Credits, Lyrics};

const DEFAULT_FORMAT: [&str; 10] = [
    "layer", "start", "end", "style", "name", "marginl", "marginr", "marginv", "effect", "text",
];

fn karaoke_durations(block: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut rest = block;
    while let Some(i) = rest.find('\\') {
        rest = &rest[i + 1..];
        let after = if let Some(r) = rest.strip_prefix("kf").or_else(|| rest.strip_prefix("ko")) {
            r
        } else if let Some(r) = rest.strip_prefix('k').or_else(|| rest.strip_prefix('K')) {
            r
        } else {
            continue;
        };
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if let Ok(cs) = digits.parse::<f64>() {
            out.push(cs / 100.0);
        }
    }
    out
}

fn dialogue_text(raw: &str, start: f64) -> (String, Vec<(f64, String)>) {
    let mut segments: Vec<(f64, String)> = Vec::new();
    let mut current = (start, String::new());
    let mut cursor = start;
    let mut timed = false;
    let mut rest = raw;
    while let Some(ch) = rest.chars().next() {
        if ch == '{'
            && let Some(close) = rest.find('}')
        {
            for d in karaoke_durations(&rest[1..close]) {
                timed = true;
                segments.push(std::mem::replace(&mut current, (cursor, String::new())));
                cursor += d;
            }
            rest = &rest[close + 1..];
            continue;
        }
        if ch == '\\' {
            let next = rest[1..].chars().next();
            if matches!(next, Some('N') | Some('n') | Some('h')) {
                current.1.push(' ');
                rest = &rest[2..];
                continue;
            }
        }
        current.1.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    segments.push(current);
    let segments: Vec<(f64, String)> = segments.into_iter().filter(|(_, s)| !s.is_empty()).collect();
    let plain = cue::squash(&segments.iter().map(|(_, s)| s.as_str()).collect::<String>());
    (plain, if timed { segments } else { Vec::new() })
}

pub fn cues(input: &str) -> Vec<Cue> {
    let mut format: Vec<String> = DEFAULT_FORMAT.iter().map(|s| s.to_string()).collect();
    let mut in_events = false;
    let mut out = Vec::new();
    for line in input.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_events = line.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim().to_ascii_lowercase();
        if key == "format" {
            format = value.split(',').map(|f| f.trim().to_ascii_lowercase()).collect();
            continue;
        }
        if key != "dialogue" {
            continue;
        }
        let fields: Vec<&str> = value.trim_start().splitn(format.len(), ',').collect();
        let get = |name: &str| format.iter().position(|f| f == name).and_then(|i| fields.get(i)).map(|s| s.trim());
        let effect = get("effect").unwrap_or("").to_ascii_lowercase();
        if ["fx", "template", "code"].iter().any(|k| effect.contains(k)) {
            continue;
        }
        let (Some(start), Some(end)) = (get("start").and_then(parse_timestamp), get("end").and_then(parse_timestamp)) else {
            continue;
        };
        let raw = format
            .iter()
            .position(|f| f == "text")
            .and_then(|i| fields.get(i))
            .copied()
            .unwrap_or("");
        let (text, segments) = dialogue_text(raw, start);
        out.push(Cue {
            start,
            end,
            text,
            segments,
            voice: get("name").or_else(|| get("actor")).filter(|n| !n.is_empty()).map(str::to_string),
        });
    }
    out
}

pub fn parse(input: &str, credits: Credits) -> Option<Lyrics> {
    cue::build(cues(input), credits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::SyncKind;

    const DOC: &str = "[Script Info]\nTitle: x\n\n[V4+ Styles]\nFormat: Name, Fontname\nStyle: Default,Arial\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nComment: 0,0:00:00.00,0:00:05.00,Default,,0,0,0,template,{\\k10}x\nDialogue: 0,0:00:01.00,0:00:03.00,Default,Ann,0,0,0,,{\\k50}Hel{\\k50}lo, {\\kf100}world\nDialogue: 0,0:00:04.00,0:00:05.00,Default,Bob,0,0,0,,{\\an8}Plain\\Nline\n";

    #[test]
    fn parses_karaoke_tags() {
        let l = parse(DOC, Credits::default()).unwrap();
        assert_eq!(l.kind, SyncKind::Syllable);
        assert_eq!(l.lines.len(), 2);
        let s = &l.lines[0].lead.syllables;
        assert_eq!(s.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["Hel", "lo,", "world"]);
        assert!((s[1].start - 1.5).abs() < 1e-9);
        assert!((s[2].start - 2.0).abs() < 1e-9);
        assert_eq!(l.lines[1].lead.text(), "Plain line");
        assert!(l.lines[1].opposite);
    }
}
