use super::{Credits, LyricLine, Lyrics, Syllable, SyncKind, VocalGroup};

fn parse_timestamp(s: &str) -> Option<f64> {
    let s = s.trim();
    let mut parts = s.split(':');
    let a = parts.next()?;
    let b = parts.next()?;
    let c = parts.next();
    let (h, m, sec) = match c {
        Some(c) => (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?, c),
        None => (0.0, a.parse::<f64>().ok()?, b),
    };
    let sec = sec.replace(',', ".");
    let sec: f64 = sec.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

struct RawLine {
    time: f64,
    words: Vec<(f64, String)>,
    text: String,
}

pub fn parse(input: &str, credits: Credits, duration: Option<f64>) -> Option<Lyrics> {
    let mut offset = 0.0;
    let mut raw: Vec<RawLine> = Vec::new();
    for line in input.lines() {
        let mut rest = line.trim_end_matches('\r');
        let mut times = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(close) = stripped.find(']') else { break };
            let tag = &stripped[..close];
            if let Some(t) = parse_timestamp(tag) {
                times.push(t);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset = v.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
            }
            rest = &stripped[close + 1..];
        }
        if times.is_empty() {
            continue;
        }
        let (words, text) = parse_enhanced(rest);
        for t in times {
            raw.push(RawLine {
                time: t,
                words: words.clone(),
                text: text.clone(),
            });
        }
    }
    if raw.is_empty() {
        return None;
    }
    raw.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    let has_words = raw.iter().any(|r| r.words.len() > 1);
    let mut lines = Vec::new();
    for i in 0..raw.len() {
        let r = &raw[i];
        if r.text.trim().is_empty() {
            continue;
        }
        let start = (r.time - offset).max(0.0);
        let next = raw.get(i + 1).map(|n| (n.time - offset).max(start));
        let end = next.unwrap_or_else(|| {
            let fallback = start + 5.0;
            duration.map(|d| d.min(fallback).max(start + 0.5)).unwrap_or(fallback)
        });
        let lead = if has_words && !r.words.is_empty() {
            build_word_group(&r.words, start, end, offset)
        } else {
            VocalGroup::single(r.text.trim().to_string(), start, end)
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
        kind: if has_words {
            SyncKind::Syllable
        } else {
            SyncKind::Line
        },
        lines,
        credits,
        language: None,
        instrumental: false,
    })
}

fn parse_enhanced(text: &str) -> (Vec<(f64, String)>, String) {
    let mut words: Vec<(f64, String)> = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    let mut current: Option<(f64, String)> = None;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix('<')
            && let Some(close) = stripped.find('>')
            && let Some(t) = parse_timestamp(&stripped[..close])
        {
            if let Some(w) = current.take() {
                words.push(w);
            }
            current = Some((t, String::new()));
            rest = &stripped[close + 1..];
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        plain.push(ch);
        if let Some((_, w)) = current.as_mut() {
            w.push(ch);
        }
        rest = &rest[ch.len_utf8()..];
    }
    if let Some(w) = current.take() {
        words.push(w);
    }
    (words, plain)
}

fn build_word_group(words: &[(f64, String)], start: f64, end: f64, offset: f64) -> VocalGroup {
    let mut syllables = Vec::new();
    for (i, (t, text)) in words.iter().enumerate() {
        let s = (t - offset).max(start);
        let e = words
            .get(i + 1)
            .map(|(n, _)| (n - offset).max(s))
            .unwrap_or(end);
        let part_of_word = !text.ends_with(char::is_whitespace);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            if let Some(last) = syllables.last_mut() {
                let last: &mut Syllable = last;
                last.part_of_word = false;
            }
            continue;
        }
        syllables.push(Syllable {
            text: trimmed.to_string(),
            start: s,
            end: e,
            part_of_word,
            roman: None,
            roman_part_of_word: false,
        });
    }
    if let Some(last) = syllables.last_mut() {
        last.part_of_word = false;
    }
    VocalGroup {
        start,
        end: syllables.last().map(|s| s.end).unwrap_or(end),
        syllables,
        roman: None,
        translation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_line_lrc() {
        let src = "[ar:x]\n[00:01.00]alpha beta\n[00:03.50]\n[00:05.00]gamma\n";
        let l = parse(src, Credits::default(), Some(10.0)).unwrap();
        assert_eq!(l.kind, SyncKind::Line);
        assert_eq!(l.lines.len(), 2);
        assert!((l.lines[0].end - 3.5).abs() < 1e-9);
        assert!((l.lines[1].end - 10.0).abs() < 1e-9);
    }

    #[test]
    fn parses_enhanced_lrc() {
        let src = "[00:01.00]<00:01.00>al<00:01.50>pha <00:02.00>beta\n[00:04.00]x";
        let l = parse(src, Credits::default(), None).unwrap();
        assert_eq!(l.kind, SyncKind::Syllable);
        let s = &l.lines[0].lead.syllables;
        assert_eq!(s.len(), 3);
        assert!(s[0].part_of_word);
        assert!(!s[1].part_of_word);
        assert_eq!(l.lines[0].lead.text(), "alpha beta");
    }
}
