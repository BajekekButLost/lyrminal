use super::cue::{self, Cue};
use super::lrc::parse_timestamp;
use super::{Credits, Lyrics};

fn first_token(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

fn cue_timing(line: &str) -> Option<(f64, f64)> {
    if let Some((a, b)) = line.split_once("-->") {
        return Some((parse_timestamp(first_token(a))?, parse_timestamp(first_token(b))?));
    }
    sbv_timing(line)
}

pub fn sbv_timing(line: &str) -> Option<(f64, f64)> {
    let (a, b) = line.trim().split_once(',')?;
    if !a.contains(':') || !b.contains(':') || b.contains(',') {
        return None;
    }
    Some((parse_timestamp(a)?, parse_timestamp(b)?))
}

struct Parsed {
    plain: String,
    segments: Vec<(f64, String)>,
    voice: Option<String>,
}

fn voice_of(tag: &str) -> Option<&str> {
    let rest = tag.strip_prefix('v')?;
    let name = if let Some(r) = rest.strip_prefix(' ') {
        r
    } else if rest.starts_with('.') {
        rest.split_once(' ').map(|(_, n)| n)?
    } else {
        return None;
    };
    let name = name.trim();
    (!name.is_empty()).then_some(name)
}

fn parse_line(line: &str, start: f64) -> Parsed {
    let mut segments: Vec<(f64, String)> = Vec::new();
    let mut current = (start, String::new());
    let mut voice = None;
    let mut timed = false;
    let mut rest = line;
    while let Some(ch) = rest.chars().next() {
        if ch == '<'
            && let Some(close) = rest.find('>')
        {
            let tag = rest[1..close].trim();
            if let Some(t) = parse_timestamp(tag) {
                timed = true;
                segments.push(std::mem::replace(&mut current, (t, String::new())));
            } else if let Some(v) = voice_of(tag) {
                voice = Some(v.to_string());
            } else if tag.to_ascii_lowercase().starts_with("br") {
                current.1.push(' ');
            }
            rest = &rest[close + 1..];
            continue;
        }
        if ch == '{'
            && rest[1..].starts_with('\\')
            && let Some(close) = rest.find('}')
        {
            rest = &rest[close + 1..];
            continue;
        }
        current.1.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    segments.push(current);
    let segments: Vec<(f64, String)> = segments
        .into_iter()
        .map(|(t, s)| (t, cue::decode_entities(&s)))
        .filter(|(_, s)| !s.is_empty())
        .collect();
    let plain = cue::squash(&segments.iter().map(|(_, s)| s.as_str()).collect::<String>());
    Parsed {
        plain,
        segments: if timed { segments } else { Vec::new() },
        voice,
    }
}

pub fn cues(input: &str) -> Vec<Cue> {
    let text = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: Vec<Cue> = Vec::new();
    let mut prev_last = String::new();
    let mut block: Vec<&str> = Vec::new();
    for line in text.lines().chain(std::iter::once("")) {
        if !line.trim().is_empty() {
            block.push(line);
            continue;
        }
        if block.is_empty() {
            continue;
        }
        let taken = std::mem::take(&mut block);
        let Some(ti) = taken.iter().position(|l| cue_timing(l).is_some()) else { continue };
        let Some((start, end)) = cue_timing(taken[ti]) else { continue };
        let parsed: Vec<Parsed> = taken[ti + 1..]
            .iter()
            .map(|l| parse_line(l, start))
            .filter(|p| !p.plain.is_empty())
            .collect();
        let Some(last) = parsed.last().map(|p| p.plain.clone()) else { continue };
        let skip = if parsed.len() > 1 {
            parsed.iter().take_while(|p| p.plain == prev_last).count().min(parsed.len() - 1)
        } else if end - start < 0.05 && parsed[0].plain == prev_last {
            1
        } else {
            0
        };
        prev_last = last;
        let kept = &parsed[skip..];
        if kept.is_empty() {
            continue;
        }
        let mut segments: Vec<(f64, String)> = Vec::new();
        if kept.iter().any(|p| !p.segments.is_empty()) {
            for (i, p) in kept.iter().enumerate() {
                let mut segs = if p.segments.is_empty() {
                    vec![(start, p.plain.clone())]
                } else {
                    p.segments.clone()
                };
                if i > 0
                    && let Some(first) = segs.first_mut()
                {
                    first.1.insert(0, ' ');
                }
                segments.extend(segs);
            }
        }
        out.push(Cue {
            start,
            end,
            text: kept.iter().map(|p| p.plain.as_str()).collect::<Vec<_>>().join(" "),
            segments,
            voice: kept.iter().find_map(|p| p.voice.clone()),
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

    #[test]
    fn parses_srt() {
        let src = "1\r\n00:00:01,000 --> 00:00:03,500\r\n<i>First</i> line\r\nwrapped\r\n\r\n2\r\n00:00:04,000 --> 00:00:05,000\r\nSecond &amp; last\r\n";
        let l = parse(src, Credits::default()).unwrap();
        assert_eq!(l.kind, SyncKind::Line);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].lead.text(), "First line wrapped");
        assert!((l.lines[0].end - 3.5).abs() < 1e-9);
        assert_eq!(l.lines[1].lead.text(), "Second & last");
    }

    #[test]
    fn parses_vtt_words_and_voices() {
        let src = "WEBVTT\n\nNOTE skip me\n\nintro\n00:01.000 --> 00:03.000 align:start\n<v Ann>hel<00:01.500>lo <00:02.000>world\n\n00:04.000 --> 00:05.000\n<v Bob>reply\n\n00:06.000 --> 00:07.000\n<v Ann>again\n";
        let l = parse(src, Credits::default()).unwrap();
        assert_eq!(l.kind, SyncKind::Syllable);
        assert_eq!(l.lines.len(), 3);
        let s = &l.lines[0].lead.syllables;
        assert_eq!(s.len(), 3);
        assert!(s[0].part_of_word);
        assert_eq!(l.lines[0].lead.text(), "hello world");
        assert!(!l.lines[0].opposite);
        assert!(l.lines[1].opposite);
    }

    #[test]
    fn drops_rolling_caption_repeats() {
        let src = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\none\n\n00:00:02.000 --> 00:00:02.010\none\n\n00:00:02.010 --> 00:00:03.000\none\ntwo\n";
        let l = parse(src, Credits::default()).unwrap();
        let texts: Vec<String> = l.lines.iter().map(|x| x.lead.text()).collect();
        assert_eq!(texts, ["one", "two"]);
    }

    #[test]
    fn parses_sbv() {
        let src = "0:00:01.000,0:00:02.500\nHello there\n\n0:00:03.000,0:00:04.000\nAgain\n";
        let l = parse(src, Credits::default()).unwrap();
        assert_eq!(l.lines.len(), 2);
        assert!((l.lines[0].end - 2.5).abs() < 1e-9);
    }
}
