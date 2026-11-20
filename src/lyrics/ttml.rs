use std::collections::HashMap;

use quick_xml::{Reader, XmlVersion};
use quick_xml::events::{BytesStart, Event};

use super::{Credits, LyricLine, Lyrics, Syllable, SyncKind, VocalGroup};

pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        return ms.trim().parse::<f64>().ok().map(|v| v / 1000.0);
    }
    if let Some(sec) = s.strip_suffix('s') {
        return sec.trim().parse::<f64>().ok();
    }
    let mut total = 0.0;
    for part in s.split(':') {
        total = total * 60.0 + part.trim().parse::<f64>().ok()?;
    }
    Some(total)
}

fn attr(e: &BytesStart, local: &str) -> Option<String> {
    for a in e.attributes().with_checks(false).flatten() {
        if a.key.local_name().as_ref() == local {
            return a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned());
        }
    }
    None
}

fn resolve_ref(name: &str) -> String {
    match name {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        _ => {
            let code = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                name.strip_prefix('#').and_then(|d| d.parse().ok())
            };
            code.and_then(char::from_u32).map(String::from).unwrap_or_default()
        }
    }
}

enum Span {
    Background,
    Syllable { begin: f64, end: f64, text: String },
    Other,
}

enum Capture {
    None,
    Songwriter,
    Translation(String),
    Transliteration(String),
}

#[derive(Default)]
struct LineBuild {
    begin: f64,
    end: f64,
    agent: Option<String>,
    key: Option<String>,
    text: String,
    lead: Vec<Syllable>,
    background: Vec<Vec<Syllable>>,
}

pub fn parse(doc: &str, credits: Credits) -> Option<Lyrics> {
    let mut reader = Reader::from_str(doc);
    let mut timing: Option<SyncKind> = None;
    let mut language: Option<String> = None;
    let mut agents: Vec<(String, String)> = Vec::new();
    let mut songwriters: Vec<String> = Vec::new();
    let mut translations: HashMap<String, String> = HashMap::new();
    let mut transliterations: HashMap<String, String> = HashMap::new();
    let mut capture = Capture::None;
    let mut capture_text = String::new();
    let mut in_translations = false;
    let mut in_transliterations = false;
    let mut line: Option<LineBuild> = None;
    let mut spans: Vec<Span> = Vec::new();
    let mut built: Vec<LineBuild> = Vec::new();

    let push_text = |text: &str, line: &mut Option<LineBuild>, spans: &mut Vec<Span>| {
        let Some(l) = line.as_mut() else { return };
        l.text.push_str(text);
        let in_bg = spans.iter().any(|s| matches!(s, Span::Background));
        match spans.last_mut() {
            Some(Span::Syllable { text: t, .. }) => t.push_str(text),
            _ => {
                if text.chars().any(char::is_whitespace) {
                    let group = if in_bg { l.background.last_mut() } else { Some(&mut l.lead) };
                    if let Some(last) = group.and_then(|g| g.last_mut()) {
                        last.part_of_word = false;
                    }
                }
            }
        }
    };

    loop {
        let event = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(e) => e,
            Err(_) => return None,
        };
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.local_name();
                match name.as_ref() {
                    "tt" => {
                        timing = attr(&e, "timing").and_then(|t| match t.to_lowercase().as_str() {
                            "word" | "syllable" => Some(SyncKind::Syllable),
                            "line" => Some(SyncKind::Line),
                            "none" => Some(SyncKind::Static),
                            _ => None,
                        });
                        language = attr(&e, "lang");
                    }
                    "agent" => {
                        if let Some(id) = attr(&e, "id") {
                            agents.push((id, attr(&e, "type").unwrap_or_default()));
                        }
                    }
                    "songwriter" => {
                        capture = Capture::Songwriter;
                        capture_text.clear();
                    }
                    "translations" => in_translations = true,
                    "transliterations" => in_transliterations = true,
                    "text" if in_translations || in_transliterations => {
                        if let Some(key) = attr(&e, "for") {
                            capture = if in_translations {
                                Capture::Translation(key)
                            } else {
                                Capture::Transliteration(key)
                            };
                            capture_text.clear();
                        }
                    }
                    "p" => {
                        let begin = attr(&e, "begin").and_then(|t| parse_time(&t)).unwrap_or(0.0);
                        let end = attr(&e, "end").and_then(|t| parse_time(&t)).unwrap_or(begin);
                        line = Some(LineBuild {
                            begin,
                            end,
                            agent: attr(&e, "agent"),
                            key: attr(&e, "key"),
                            ..Default::default()
                        });
                        spans.clear();
                    }
                    "span" if line.is_some() && matches!(capture, Capture::None) => {
                        if attr(&e, "role").as_deref() == Some("x-bg") {
                            if let Some(l) = line.as_mut() {
                                l.background.push(Vec::new());
                            }
                            spans.push(Span::Background);
                        } else if let Some(begin) = attr(&e, "begin").and_then(|t| parse_time(&t)) {
                            let end = attr(&e, "end").and_then(|t| parse_time(&t)).unwrap_or(begin);
                            spans.push(Span::Syllable {
                                begin,
                                end,
                                text: String::new(),
                            });
                        } else {
                            spans.push(Span::Other);
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                let text = t.xml10_content().into_owned();
                if !matches!(capture, Capture::None) {
                    capture_text.push_str(&text);
                } else {
                    push_text(&text, &mut line, &mut spans);
                }
            }
            Event::CData(t) => {
                let text = t.xml10_content().into_owned();
                if !matches!(capture, Capture::None) {
                    capture_text.push_str(&text);
                } else {
                    push_text(&text, &mut line, &mut spans);
                }
            }
            Event::GeneralRef(r) => {
                let text = resolve_ref(&r.xml10_content());
                if !matches!(capture, Capture::None) {
                    capture_text.push_str(&text);
                } else {
                    push_text(&text, &mut line, &mut spans);
                }
            }
            Event::End(e) => {
                let name = e.local_name();
                match name.as_ref() {
                    "songwriter" => {
                        let t = capture_text.trim().to_string();
                        if !t.is_empty() {
                            songwriters.push(t);
                        }
                        capture = Capture::None;
                    }
                    "translations" => in_translations = false,
                    "transliterations" => in_transliterations = false,
                    "text" if !matches!(capture, Capture::None) => {
                        let t = capture_text.split_whitespace().collect::<Vec<_>>().join(" ");
                        match std::mem::replace(&mut capture, Capture::None) {
                            Capture::Translation(k) => {
                                translations.insert(k, t);
                            }
                            Capture::Transliteration(k) => {
                                transliterations.insert(k, t);
                            }
                            _ => {}
                        }
                    }
                    "span" if line.is_some() => match spans.pop() {
                        Some(Span::Syllable { begin, end, text }) => {
                            let in_bg = spans.iter().any(|s| matches!(s, Span::Background));
                            let trimmed = text.trim();
                            if let Some(l) = line.as_mut() {
                                let group = if in_bg { l.background.last_mut() } else { Some(&mut l.lead) };
                                if let Some(g) = group {
                                    if trimmed.is_empty() {
                                        if let Some(last) = g.last_mut() {
                                            last.part_of_word = false;
                                        }
                                    } else {
                                        if text.starts_with(char::is_whitespace)
                                            && let Some(last) = g.last_mut()
                                        {
                                            last.part_of_word = false;
                                        }
                                        g.push(Syllable {
                                            text: trimmed.to_string(),
                                            start: begin,
                                            end,
                                            part_of_word: !text.ends_with(char::is_whitespace),
                                            roman: None,
                                            roman_part_of_word: false,
                                        });
                                    }
                                }
                            }
                        }
                        Some(Span::Background) => {
                            if let Some(last) = line
                                .as_mut()
                                .and_then(|l| l.background.last_mut())
                                .and_then(|g| g.last_mut())
                            {
                                last.part_of_word = false;
                            }
                        }
                        _ => {}
                    },
                    "p" => {
                        if let Some(mut l) = line.take() {
                            if let Some(last) = l.lead.last_mut() {
                                last.part_of_word = false;
                            }
                            built.push(l);
                        }
                        spans.clear();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let timing = timing.unwrap_or_else(|| {
        if built.iter().any(|l| !l.lead.is_empty()) {
            SyncKind::Syllable
        } else if built.iter().any(|l| l.end > 0.0) {
            SyncKind::Line
        } else {
            SyncKind::Static
        }
    });

    let is_person = |a: &str| agents.iter().find(|(id, _)| id == a).map(|(_, t)| t != "group").unwrap_or(true);
    let declared = agents
        .iter()
        .filter(|(_, t)| t != "group")
        .map(|(id, _)| id.clone())
        .find(|id| built.iter().any(|l| l.agent.as_deref() == Some(id.as_str())));
    let main_agent = declared.or_else(|| {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for l in built.iter().filter(|l| !l.lead.is_empty()) {
            if let Some(a) = l.agent.as_deref().filter(|a| is_person(a)) {
                match counts.iter_mut().find(|(id, _)| id == a) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((a.to_string(), 1)),
                }
            }
        }
        counts.into_iter().max_by_key(|(_, n)| *n).map(|(id, _)| id)
    });
    let is_opposite = |agent: &Option<String>| -> bool {
        let (Some(a), Some(main)) = (agent, &main_agent) else { return false };
        if a == main {
            return false;
        }
        agents
            .iter()
            .find(|(id, _)| id == a)
            .map(|(_, t)| t != "group")
            .unwrap_or(true)
    };

    let mut lines = Vec::new();
    for l in built {
        let opposite = is_opposite(&l.agent);
        let translation = l.key.as_ref().and_then(|k| translations.get(k).cloned());
        let roman = l.key.as_ref().and_then(|k| transliterations.get(k).cloned());
        let plain = l.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut lead = match timing {
            SyncKind::Syllable if !l.lead.is_empty() => VocalGroup {
                start: l.lead.first().map(|s| s.start).unwrap_or(l.begin),
                end: l.lead.last().map(|s| s.end).unwrap_or(l.end),
                syllables: l.lead,
                roman: None,
                translation: None,
            },
            SyncKind::Syllable if l.background.iter().any(|g| !g.is_empty()) => VocalGroup::default(),
            SyncKind::Static => {
                if plain.is_empty() {
                    continue;
                }
                VocalGroup::single(plain.clone(), 0.0, 0.0)
            }
            _ => {
                if plain.is_empty() {
                    continue;
                }
                VocalGroup::single(plain.clone(), l.begin, l.end)
            }
        };
        lead.roman = roman;
        lead.translation = translation;
        let background: Vec<VocalGroup> = if timing == SyncKind::Syllable {
            l.background
                .into_iter()
                .filter(|g| !g.is_empty())
                .map(|g| VocalGroup {
                    start: g.first().map(|s| s.start).unwrap_or(0.0),
                    end: g.last().map(|s| s.end).unwrap_or(0.0),
                    syllables: g,
                    roman: None,
                    translation: None,
                })
                .collect()
        } else {
            Vec::new()
        };
        let (start, end) = if timing == SyncKind::Static {
            (0.0, 0.0)
        } else {
            (lead.start.min(l.begin.max(0.0)), lead.end.max(l.end))
        };
        lines.push(LyricLine {
            start,
            end,
            opposite,
            lead,
            background,
            background_only: false,
        });
    }
    if lines.is_empty() {
        return None;
    }
    let mut credits = credits;
    if credits.songwriters.is_empty() {
        credits.songwriters = songwriters;
    }
    let mut lyrics = Lyrics {
        kind: timing,
        lines,
        credits,
        language,
        instrumental: false,
    };
    lyrics.sort();
    Some(lyrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:lrc="x" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" lrc:timing="Word" xml:lang="en"><head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/><sourceMetadata><songwriters><songwriter>Writer One</songwriter></songwriters></sourceMetadata></metadata></head><body><div><p begin="1.0" end="3.0" lrc:key="L1" ttm:agent="v1"><span ttm:role="x-bg"><span begin="1.0" end="1.2">(oh</span><span begin="1.2" end="1.4">h)</span></span> <span begin="1.5" end="1.8">rock</span><span begin="1.8" end="2.0">&amp;</span> <span begin="2.0" end="3.0">roll</span></p><p begin="4.0" end="5.0" lrc:key="L2" ttm:agent="v2"><span begin="4.0" end="5.0">duet</span></p></div></body></tt>"#;

    #[test]
    fn parses_word_ttml() {
        let l = parse(DOC, Credits::default()).unwrap();
        assert_eq!(l.kind, SyncKind::Syllable);
        assert_eq!(l.lines.len(), 2);
        let first = &l.lines[0];
        assert_eq!(first.lead.text(), "rock& roll");
        assert_eq!(first.background.len(), 1);
        assert_eq!(first.background[0].text(), "(ohh)");
        assert!(!first.opposite);
        assert!(l.lines[1].opposite);
        assert_eq!(l.credits.songwriters, vec!["Writer One".to_string()]);
        assert_eq!(l.language.as_deref(), Some("en"));
    }

    #[test]
    fn promotes_background_only_duet_lines() {
        let doc = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xmlns:lrc="x" lrc:timing="Word"><head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/></metadata></head><body><div><p begin="0.4" end="1.5" ttm:agent="v2"><span ttm:role="x-bg"><span begin="0.4" end="0.9">(ah</span> <span begin="0.9" end="1.5">ah)</span></span></p><p begin="2.0" end="3.0" ttm:agent="v1"><span begin="2.0" end="3.0">main</span></p></div></body></tt>"#;
        let l = parse(doc, Credits::default()).unwrap();
        assert_eq!(l.lines.len(), 2);
        let first = &l.lines[0];
        assert!(first.background_only);
        assert!(first.opposite);
        assert!(first.background.is_empty());
        assert_eq!(first.lead.text(), "(ah ah)");
        assert!((first.start - 0.4).abs() < 1e-9);
        assert_eq!(first.lead.syllables.len(), 2);
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("1:02.5"), Some(62.5));
        assert_eq!(parse_time("00:01:02.500"), Some(62.5));
        assert_eq!(parse_time("2838ms"), Some(2.838));
        assert_eq!(parse_time("2.5s"), Some(2.5));
    }
}
