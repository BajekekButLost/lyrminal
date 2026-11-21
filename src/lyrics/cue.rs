use super::{Credits, LyricLine, Lyrics, Syllable, SyncKind, VocalGroup};

#[derive(Debug, Clone, Default)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub segments: Vec<(f64, String)>,
    pub voice: Option<String>,
}

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let end = tail.find(';').filter(|e| *e <= 10);
        let decoded = end.and_then(|e| {
            let name = &tail[..e];
            let ch = match name.to_ascii_lowercase().as_str() {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                lower => {
                    let code = if let Some(hex) = lower.strip_prefix("#x") {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        lower.strip_prefix('#').and_then(|d| d.parse().ok())
                    };
                    code.and_then(char::from_u32)
                }
            };
            ch.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &tail[e + 1..];
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(ch) = rest.chars().next() {
        let close = match ch {
            '<' => rest.find('>'),
            '{' if rest[1..].starts_with('\\') => rest.find('}'),
            _ => None,
        };
        match close {
            Some(c) => {
                let tag = rest[1..c].trim().to_ascii_lowercase();
                if tag.starts_with("br") {
                    out.push(' ');
                }
                rest = &rest[c + 1..];
            }
            None => {
                out.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    decode_entities(&out)
}

pub fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_words(syllables: &mut Vec<Syllable>, text: &str, start: f64, end: f64) {
    if text.starts_with(char::is_whitespace)
        && let Some(last) = syllables.last_mut()
    {
        last.part_of_word = false;
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        if let Some(last) = syllables.last_mut() {
            last.part_of_word = false;
        }
        return;
    }
    let total: usize = words.iter().map(|w| w.chars().count().max(1)).sum();
    let span = (end - start).max(0.0);
    let mut t = start;
    for (i, w) in words.iter().enumerate() {
        let share = span * w.chars().count().max(1) as f64 / total as f64;
        let last = i + 1 == words.len();
        syllables.push(Syllable {
            text: w.to_string(),
            start: t,
            end: if last { end.max(t) } else { t + share },
            part_of_word: last && !text.ends_with(char::is_whitespace),
            roman: None,
            roman_part_of_word: false,
        });
        t += share;
    }
}

pub fn group_from_segments(segments: &[(f64, String)], start: f64, end: f64) -> VocalGroup {
    let mut syllables: Vec<Syllable> = Vec::new();
    for (i, (t, text)) in segments.iter().enumerate() {
        let s = t.max(start);
        let e = segments
            .get(i + 1)
            .map(|(n, _)| n.max(s))
            .unwrap_or(end)
            .max(s);
        push_words(&mut syllables, text, s, e);
    }
    if let Some(last) = syllables.last_mut() {
        last.part_of_word = false;
    }
    VocalGroup {
        start,
        end: syllables.last().map(|s| s.end).unwrap_or(end).max(start),
        syllables,
        roman: None,
        translation: None,
    }
}

pub fn build(cues: Vec<Cue>, credits: Credits) -> Option<Lyrics> {
    let cues: Vec<Cue> = cues
        .into_iter()
        .map(|mut c| {
            c.text = squash(&c.text);
            c.end = c.end.max(c.start);
            c
        })
        .filter(|c| !c.text.is_empty())
        .collect();
    if cues.is_empty() {
        return None;
    }
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for v in cues.iter().filter_map(|c| c.voice.as_deref()) {
        match counts.iter_mut().find(|(n, _)| *n == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    let main = counts.iter().rev().max_by_key(|(_, n)| *n).map(|(v, _)| v.to_string());
    let timed_words = cues.iter().any(|c| c.segments.len() > 1);
    let lines = cues
        .iter()
        .map(|c| {
            let lead = if timed_words && !c.segments.is_empty() {
                group_from_segments(&c.segments, c.start, c.end)
            } else {
                VocalGroup::single(c.text.clone(), c.start, c.end)
            };
            LyricLine {
                start: c.start,
                end: c.end.max(lead.end),
                opposite: c.voice.is_some() && c.voice != main,
                lead,
                background: Vec::new(),
                background_only: false,
            }
        })
        .collect();
    Some(Lyrics {
        kind: if timed_words { SyncKind::Syllable } else { SyncKind::Line },
        lines,
        credits,
        language: None,
        instrumental: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markup_and_entities() {
        assert_eq!(strip_markup("<i>Rock</i> &amp; {\\an8}roll<br/>again"), "Rock & roll again");
        assert_eq!(decode_entities("a &#x41;&#66; & b"), "a AB & b");
    }

    #[test]
    fn splits_segments_into_words() {
        let g = group_from_segments(&[(1.0, "hel".into()), (1.5, "lo there ".into()), (2.0, "you".into())], 1.0, 3.0);
        let words: Vec<&str> = g.syllables.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(words, ["hel", "lo", "there", "you"]);
        assert!(g.syllables[0].part_of_word);
        assert!(!g.syllables[1].part_of_word);
        assert!(!g.syllables[2].part_of_word);
        assert!((g.syllables[3].end - 3.0).abs() < 1e-9);
    }
}
