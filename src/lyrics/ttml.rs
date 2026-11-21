use std::collections::HashMap;

use quick_xml::{Reader, XmlVersion};
use quick_xml::events::{BytesStart, Event};

use super::{Contributor, Credits, LyricLine, Lyrics, Syllable, SyncKind, VocalGroup};

#[derive(Clone, Copy)]
struct Rates {
    frame: f64,
    tick: f64,
}

impl Default for Rates {
    fn default() -> Self {
        Self { frame: 30.0, tick: 1.0 }
    }
}

#[cfg(test)]
fn parse_time(s: &str) -> Option<f64> {
    parse_time_with(s, Rates::default())
}

fn parse_time_with(s: &str, rates: Rates) -> Option<f64> {
    let s = s.trim();
    let num = |v: &str| v.trim().parse::<f64>().ok();
    if let Some(v) = s.strip_suffix("ms") {
        return num(v).map(|v| v / 1000.0);
    }
    if !s.contains(':') {
        for (suffix, scale) in [("h", 3600.0), ("m", 60.0), ("s", 1.0), ("f", 1.0 / rates.frame), ("t", 1.0 / rates.tick)] {
            if let Some(v) = s.strip_suffix(suffix) {
                return num(v).map(|v| v * scale);
            }
        }
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 4 {
        let frames = num(parts[3])?;
        return Some(num(parts[0])? * 3600.0 + num(parts[1])? * 60.0 + num(parts[2])? + frames / rates.frame);
    }
    let mut total = 0.0;
    for part in parts {
        total = total * 60.0 + num(part)?;
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

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_formatting(text: &str) -> bool {
    text.trim().is_empty() && text.contains(['\n', '\r'])
}

fn role(e: &BytesStart) -> Option<String> {
    attr(e, "role").map(|r| r.trim().to_ascii_lowercase())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub titles: Vec<String>,
    pub artists: Vec<String>,
    pub albums: Vec<String>,
    pub isrc: Option<String>,
    pub spotify_id: Option<String>,
    pub author: Option<String>,
}

pub fn metadata(doc: &str) -> Meta {
    let mut reader = Reader::from_str(doc);
    let mut meta = Meta::default();
    let mut title: Option<String> = None;
    let mut ttm_title: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) if e.local_name().as_ref() == "body" => break,
            Ok(Event::Start(e)) if e.local_name().as_ref() == "title" => title = Some(String::new()),
            Ok(Event::Text(t)) => {
                if let Some(s) = title.as_mut() {
                    s.push_str(&t.xml10_content());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Some(s) = title.as_mut() {
                    s.push_str(&resolve_ref(&r.xml10_content()));
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "title" => {
                if let Some(s) = title.take().map(|s| squash(&s)).filter(|s| !s.is_empty()) {
                    ttm_title.get_or_insert(s);
                }
            }
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.local_name().as_ref() == "meta" => {
                let (Some(key), Some(value)) = (attr(&e, "key"), attr(&e, "value")) else { continue };
                let value = value.trim().to_string();
                if value.is_empty() {
                    continue;
                }
                match key.as_str() {
                    "musicName" => meta.titles.push(value),
                    "artists" => meta.artists.push(value),
                    "album" => meta.albums.push(value),
                    "isrc" => {
                        meta.isrc.get_or_insert(value);
                    }
                    "spotifyId" => {
                        meta.spotify_id.get_or_insert(value);
                    }
                    "ttmlAuthorGithubLogin" => {
                        meta.author.get_or_insert(value);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if let Some(t) = ttm_title
        && !meta.titles.contains(&t)
    {
        meta.titles.insert(0, t);
    }
    meta
}

#[derive(Clone, Copy, PartialEq)]
enum Aux {
    Translation,
    Roman,
}

enum Span {
    Background,
    Syllable { begin: f64, end: f64, text: String },
    Aux { kind: Aux, text: String },
    Other,
}

enum Capture {
    None,
    Songwriter,
}

#[derive(Default)]
struct BgBuild {
    syllables: Vec<Syllable>,
    text: String,
    translation: Option<String>,
    roman: Option<String>,
}

#[derive(Default)]
struct LineBuild {
    begin: f64,
    end: f64,
    agent: Option<String>,
    key: Option<String>,
    text: String,
    lead: Vec<Syllable>,
    background: Vec<BgBuild>,
    translation: Option<String>,
    roman: Option<String>,
}

#[derive(Default)]
struct AuxText {
    main: String,
    bg: String,
    syllables: Vec<String>,
    bg_syllables: Vec<String>,
    timed: bool,
}

enum CapSpan {
    Background,
    Timed,
    Other,
}

struct TextCapture {
    kind: Aux,
    key: String,
    text: AuxText,
    stack: Vec<CapSpan>,
}

fn push_line_text(text: &str, line: &mut Option<LineBuild>, spans: &mut [Span]) {
    let Some(l) = line.as_mut() else { return };
    if let Some(Span::Aux { text: t, .. }) = spans.iter_mut().rev().find(|s| matches!(s, Span::Aux { .. })) {
        t.push_str(text);
        return;
    }
    let in_bg = spans.iter().any(|s| matches!(s, Span::Background));
    if in_bg {
        if let Some(b) = l.background.last_mut() {
            b.text.push_str(text);
        }
    } else {
        l.text.push_str(text);
    }
    match spans.last_mut() {
        Some(Span::Syllable { text: t, .. }) => t.push_str(text),
        _ if is_formatting(text) => {}
        _ => {
            if text.chars().any(char::is_whitespace) {
                let group = if in_bg {
                    l.background.last_mut().map(|b| &mut b.syllables)
                } else {
                    Some(&mut l.lead)
                };
                if let Some(last) = group.and_then(|g| g.last_mut()) {
                    last.part_of_word = false;
                }
            }
        }
    }
}

fn push_capture_text(text: &str, cap: &mut TextCapture) {
    let in_bg = cap.stack.iter().any(|s| matches!(s, CapSpan::Background));
    let t = &mut cap.text;
    if in_bg {
        t.bg.push_str(text);
    } else {
        t.main.push_str(text);
    }
    let list = if in_bg { &mut t.bg_syllables } else { &mut t.syllables };
    if matches!(cap.stack.last(), Some(CapSpan::Timed)) {
        if let Some(last) = list.last_mut() {
            last.push_str(text);
        }
    } else if !is_formatting(text)
        && text.chars().any(char::is_whitespace)
        && let Some(last) = list.last_mut()
        && !last.ends_with(char::is_whitespace)
    {
        last.push(' ');
    }
}

fn apply_roman(group: &mut [Syllable], raws: &[String]) -> bool {
    let n = group.len();
    if n == 0 || raws.len() != n || raws.iter().any(|r| r.trim().is_empty()) {
        return false;
    }
    for (i, (s, raw)) in group.iter_mut().zip(raws).enumerate() {
        let next_spaced = raws.get(i + 1).is_some_and(|r| r.starts_with(char::is_whitespace));
        s.roman = Some(raw.trim().to_string());
        s.roman_part_of_word = i + 1 < n && !raw.ends_with(char::is_whitespace) && !next_spaced;
    }
    true
}

const OPEN: [char; 2] = ['(', '（'];
const CLOSE: [char; 2] = [')', '）'];

fn paren_edges(s: &str) -> (bool, bool) {
    let s = s.trim();
    let opens = s.starts_with(OPEN);
    let closes = s.ends_with(CLOSE);
    if opens && closes && s.chars().count() >= 2 {
        let inner: Vec<char> = s.chars().skip(1).collect();
        let mut depth = 0i32;
        let balanced = inner[..inner.len() - 1].iter().all(|c| {
            if OPEN.contains(c) {
                depth += 1;
            } else if CLOSE.contains(c) {
                depth -= 1;
            }
            depth >= 0
        }) && depth == 0;
        return (balanced, balanced);
    }
    (opens && !s.contains(CLOSE), closes && !s.contains(OPEN))
}

fn strip_edges(s: &mut String, start: bool, end: bool) {
    let mut t = s.trim();
    if start {
        t = t.strip_prefix(OPEN).unwrap_or(t).trim_start();
    }
    if end {
        t = t.strip_suffix(CLOSE).unwrap_or(t).trim_end();
    }
    *s = t.to_string();
}

fn strip_parens(g: &mut VocalGroup) {
    let joined: String = g.syllables.iter().map(|s| s.text.as_str()).collect();
    let (start, end) = paren_edges(&joined);
    if let Some(first) = g.syllables.first_mut() {
        strip_edges(&mut first.text, start, false);
    }
    if let Some(last) = g.syllables.last_mut() {
        strip_edges(&mut last.text, false, end);
    }
    g.syllables.retain(|s| !s.text.is_empty());
    if g.syllables.iter().all(|s| s.roman.is_some()) {
        let joined: String = g.syllables.iter().filter_map(|s| s.roman.as_deref()).collect();
        let (start, end) = paren_edges(&joined);
        if let Some(r) = g.syllables.first_mut().and_then(|s| s.roman.as_mut()) {
            strip_edges(r, start, false);
        }
        if let Some(r) = g.syllables.last_mut().and_then(|s| s.roman.as_mut()) {
            strip_edges(r, false, end);
        }
    }
    for text in [&mut g.roman, &mut g.translation].into_iter().flatten() {
        let (start, end) = paren_edges(text);
        strip_edges(text, start, end);
    }
}

fn non_empty(s: &str) -> Option<String> {
    let s = squash(s);
    (!s.is_empty()).then_some(s)
}

pub fn parse(doc: &str, credits: Credits) -> Option<Lyrics> {
    let mut reader = Reader::from_str(doc);
    let mut timing: Option<SyncKind> = None;
    let mut language: Option<String> = None;
    let mut agents: Vec<(String, String)> = Vec::new();
    let mut songwriters: Vec<String> = Vec::new();
    let mut translations: HashMap<String, AuxText> = HashMap::new();
    let mut transliterations: HashMap<String, AuxText> = HashMap::new();
    let mut capture = Capture::None;
    let mut capture_text = String::new();
    let mut text_cap: Option<TextCapture> = None;
    let mut in_translations = false;
    let mut in_transliterations = false;
    let mut skip_block = false;
    let mut line: Option<LineBuild> = None;
    let mut spans: Vec<Span> = Vec::new();
    let mut built: Vec<LineBuild> = Vec::new();
    let mut rates = Rates::default();
    let span_times = |e: &BytesStart, rates: Rates| {
        let begin = attr(e, "begin").and_then(|t| parse_time_with(&t, rates));
        let end = attr(e, "end")
            .and_then(|t| parse_time_with(&t, rates))
            .or_else(|| Some(begin? + attr(e, "dur").and_then(|t| parse_time_with(&t, rates))?));
        (begin, end)
    };

    loop {
        let event = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(e) => e,
            Err(_) => return None,
        };
        let empty = matches!(event, Event::Empty(_));
        let text = match &event {
            Event::Text(t) => Some(t.xml10_content().into_owned()),
            Event::CData(t) => Some(t.xml10_content().into_owned()),
            Event::GeneralRef(r) => Some(resolve_ref(&r.xml10_content())),
            _ => None,
        };
        if let Some(text) = text {
            if let Some(cap) = text_cap.as_mut() {
                push_capture_text(&text, cap);
            } else if !matches!(capture, Capture::None) {
                capture_text.push_str(&text);
            } else {
                push_line_text(&text, &mut line, &mut spans);
            }
            continue;
        }
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.local_name();
                match name.as_ref() {
                    "tt" => {
                        timing = attr(&e, "timing").and_then(|t| match t.trim().to_lowercase().as_str() {
                            "word" | "syllable" => Some(SyncKind::Syllable),
                            "line" => Some(SyncKind::Line),
                            "none" => Some(SyncKind::Static),
                            _ => None,
                        });
                        language = attr(&e, "lang");
                        let frame = attr(&e, "frameRate").and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| *v > 0.0);
                        let multiplier = attr(&e, "frameRateMultiplier").and_then(|v| {
                            let (a, b) = v.trim().split_once(char::is_whitespace)?;
                            Some(a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok()?)
                        });
                        rates.frame = frame.unwrap_or(30.0) * multiplier.filter(|m| *m > 0.0).unwrap_or(1.0);
                        rates.tick = attr(&e, "tickRate")
                            .and_then(|v| v.trim().parse::<f64>().ok())
                            .filter(|v| *v > 0.0)
                            .unwrap_or(if frame.is_some() { rates.frame } else { 1.0 });
                    }
                    "br" => {
                        if let Some(cap) = text_cap.as_mut() {
                            push_capture_text(" ", cap);
                        } else {
                            push_line_text(" ", &mut line, &mut spans);
                        }
                    }
                    _ if empty => {
                        if name.as_ref() == "agent"
                            && let Some(id) = attr(&e, "id")
                        {
                            agents.push((id, attr(&e, "type").unwrap_or_default()));
                        }
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
                    "translation" | "transliteration" => {
                        skip_block = attr(&e, "type").is_some_and(|t| t.trim().eq_ignore_ascii_case("replacement"));
                    }
                    "text" if (in_translations || in_transliterations) && !skip_block => {
                        if let Some(key) = attr(&e, "for") {
                            text_cap = Some(TextCapture {
                                kind: if in_translations { Aux::Translation } else { Aux::Roman },
                                key,
                                text: AuxText::default(),
                                stack: Vec::new(),
                            });
                        }
                    }
                    "span" if text_cap.is_some() => {
                        let Some(cap) = text_cap.as_mut() else { continue };
                        if role(&e).as_deref() == Some("x-bg") {
                            cap.stack.push(CapSpan::Background);
                        } else if attr(&e, "begin").is_some() {
                            let in_bg = cap.stack.iter().any(|s| matches!(s, CapSpan::Background));
                            let list = if in_bg { &mut cap.text.bg_syllables } else { &mut cap.text.syllables };
                            list.push(String::new());
                            cap.text.timed = true;
                            cap.stack.push(CapSpan::Timed);
                        } else {
                            cap.stack.push(CapSpan::Other);
                        }
                    }
                    "p" => {
                        let (begin, end) = span_times(&e, rates);
                        let begin = begin.unwrap_or(0.0);
                        let end = end.unwrap_or(begin);
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
                        let span = if spans.iter().any(|s| matches!(s, Span::Aux { .. })) {
                            Span::Other
                        } else {
                            match role(&e).as_deref() {
                                Some("x-bg") => {
                                    if let Some(l) = line.as_mut() {
                                        l.background.push(BgBuild::default());
                                    }
                                    Span::Background
                                }
                                Some("x-translation") => Span::Aux {
                                    kind: Aux::Translation,
                                    text: String::new(),
                                },
                                Some("x-roman") => Span::Aux {
                                    kind: Aux::Roman,
                                    text: String::new(),
                                },
                                _ => match span_times(&e, rates) {
                                    (Some(begin), end) => Span::Syllable {
                                        begin,
                                        end: end.unwrap_or(begin),
                                        text: String::new(),
                                    },
                                    _ => Span::Other,
                                },
                            }
                        };
                        spans.push(span);
                    }
                    _ => {}
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
                    "translation" | "transliteration" => skip_block = false,
                    "text" if text_cap.is_some() => {
                        if let Some(cap) = text_cap.take() {
                            let map = match cap.kind {
                                Aux::Translation => &mut translations,
                                Aux::Roman => &mut transliterations,
                            };
                            map.entry(cap.key).or_insert(cap.text);
                        }
                    }
                    "span" if text_cap.is_some() => {
                        if let Some(cap) = text_cap.as_mut() {
                            cap.stack.pop();
                        }
                    }
                    "span" if line.is_some() => match spans.pop() {
                        Some(Span::Syllable { begin, end, text }) => {
                            let in_bg = spans.iter().any(|s| matches!(s, Span::Background));
                            let trimmed = text.trim();
                            if let Some(l) = line.as_mut() {
                                let group = if in_bg {
                                    l.background.last_mut().map(|b| &mut b.syllables)
                                } else {
                                    Some(&mut l.lead)
                                };
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
                                .and_then(|b| b.syllables.last_mut())
                            {
                                last.part_of_word = false;
                            }
                        }
                        Some(Span::Aux { kind, text }) => {
                            let in_bg = spans.iter().any(|s| matches!(s, Span::Background));
                            if let (Some(l), Some(t)) = (line.as_mut(), non_empty(&text)) {
                                let slot = match (in_bg, kind) {
                                    (true, Aux::Translation) => l.background.last_mut().map(|b| &mut b.translation),
                                    (true, Aux::Roman) => l.background.last_mut().map(|b| &mut b.roman),
                                    (false, Aux::Translation) => Some(&mut l.translation),
                                    (false, Aux::Roman) => Some(&mut l.roman),
                                };
                                if let Some(slot) = slot {
                                    slot.get_or_insert(t);
                                }
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
        if built
            .iter()
            .any(|l| !l.lead.is_empty() || l.background.iter().any(|b| !b.syllables.is_empty()))
        {
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
        let head_tr = l.key.as_ref().and_then(|k| translations.get(k));
        let head_ro = l.key.as_ref().and_then(|k| transliterations.get(k));
        let plain = squash(&l.text);
        let has_bg = l.background.iter().any(|b| !b.syllables.is_empty() || !squash(&b.text).is_empty());
        let (line_start, line_end) = if timing == SyncKind::Static { (0.0, 0.0) } else { (l.begin, l.end) };
        let mut lead = match timing {
            SyncKind::Syllable if !l.lead.is_empty() => VocalGroup {
                start: l.lead.first().map(|s| s.start).unwrap_or(l.begin),
                end: l.lead.last().map(|s| s.end).unwrap_or(l.end),
                syllables: l.lead,
                roman: None,
                translation: None,
            },
            SyncKind::Syllable if l.background.iter().any(|g| !g.syllables.is_empty()) => VocalGroup::default(),
            _ if plain.is_empty() && has_bg && timing != SyncKind::Syllable => VocalGroup::default(),
            SyncKind::Syllable if plain.is_empty() => continue,
            _ => {
                if plain.is_empty() {
                    continue;
                }
                VocalGroup::single(plain.clone(), line_start, line_end)
            }
        };
        lead.translation = l.translation.or_else(|| head_tr.and_then(|t| non_empty(&t.main)));
        let timed_roman = timing == SyncKind::Syllable
            && head_ro.is_some_and(|r| r.timed && apply_roman(&mut lead.syllables, &r.syllables));
        if !timed_roman {
            lead.roman = l.roman.or_else(|| head_ro.and_then(|r| non_empty(&r.main)));
        }
        let mut background: Vec<VocalGroup> = Vec::new();
        for (i, b) in l.background.into_iter().enumerate() {
            let mut g = if timing == SyncKind::Syllable {
                if b.syllables.is_empty() {
                    continue;
                }
                VocalGroup {
                    start: b.syllables.first().map(|s| s.start).unwrap_or(0.0),
                    end: b.syllables.last().map(|s| s.end).unwrap_or(0.0),
                    syllables: b.syllables,
                    roman: None,
                    translation: None,
                }
            } else {
                let Some(t) = non_empty(&b.text) else { continue };
                VocalGroup::single(t, line_start, line_end)
            };
            let first = i == 0;
            g.translation = b
                .translation
                .or_else(|| head_tr.filter(|_| first).and_then(|t| non_empty(&t.bg)));
            let timed_bg_roman = timing == SyncKind::Syllable
                && first
                && head_ro.is_some_and(|r| r.timed && apply_roman(&mut g.syllables, &r.bg_syllables));
            if !timed_bg_roman {
                g.roman = b.roman.or_else(|| head_ro.filter(|_| first).and_then(|r| non_empty(&r.bg)));
            }
            strip_parens(&mut g);
            if g.syllables.is_empty() {
                continue;
            }
            background.push(g);
        }
        let (start, end) = if timing == SyncKind::Static {
            (0.0, 0.0)
        } else if lead.syllables.is_empty() {
            (l.begin.max(0.0), l.end)
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
    if let Some(login) = metadata(doc).author
        && !credits.contributors.iter().any(|c| c.name.eq_ignore_ascii_case(&login))
    {
        credits.contributors.push(Contributor {
            role: "Synced by".into(),
            url: Some(format!("https://github.com/{login}")),
            name: login,
        });
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
        assert_eq!(first.background[0].text(), "ohh");
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
        assert_eq!(first.lead.text(), "ah ah");
        assert!((first.start - 0.4).abs() < 1e-9);
        assert_eq!(first.lead.syllables.len(), 2);
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("1:02.5"), Some(62.5));
        assert_eq!(parse_time("00:01:02.500"), Some(62.5));
        assert_eq!(parse_time("00:02:35"), Some(155.0));
        assert_eq!(parse_time("02:35.55"), Some(155.55));
        assert_eq!(parse_time("95"), Some(95.0));
        assert_eq!(parse_time("15.8s"), Some(15.8));
        assert_eq!(parse_time("2838ms"), Some(2.838));
        assert_eq!(parse_time("00:00:01:15"), Some(1.5));
        assert_eq!(parse_time("1.5m"), Some(90.0));
    }

    #[test]
    fn parses_subtitle_ttml() {
        let doc = r#"<?xml version="1.0"?><tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttp="http://www.w3.org/ns/ttml#parameter" ttp:tickRate="10000000"><body><div><p begin="10000000t" end="25000000t">first<br/>line</p><p begin="3s" dur="1s"><span style="s1">second</span></p></div></body></tt>"#;
        let l = parse(doc, Credits::default()).unwrap();
        assert_eq!(l.kind, SyncKind::Line);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].lead.text(), "first line");
        assert!((l.lines[0].start - 1.0).abs() < 1e-9);
        assert!((l.lines[0].end - 2.5).abs() < 1e-9);
        assert!((l.lines[1].end - 4.0).abs() < 1e-9);
    }

    const AMLL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xmlns:itunes="http://itunes.apple.com/lyric-ttml-extensions" xmlns:amll="http://www.example.com/ns/amll" xml:lang="ja" itunes:timing="Word">
    <head>
        <metadata>
            <ttm:title>Song Title</ttm:title>
            <ttm:agent type="person" xml:id="v1">
                <ttm:name type="full">Artist A</ttm:name>
            </ttm:agent>
            <ttm:agent type="group" xml:id="v1000" />
            <amll:meta key="musicName" value="Song Title" />
            <amll:meta key="artists" value="Artist A" />
            <amll:meta key="album" value="Album" />
            <amll:meta key="spotifyId" value="abc123" />
            <amll:meta key="ttmlAuthorGithubLogin" value="someone" />
            <iTunesMetadata xmlns="http://music.apple.com/lyric-ttml-internal">
                <translations>
                    <translation type="replacement" xml:lang="zh-Hant">
                        <text for="L2">replaced</text>
                    </translation>
                    <translation type="subtitle" xml:lang="en">
                        <text for="L2">Head translation <span ttm:role="x-bg">(bg translation)</span></text>
                    </translation>
                    <translation type="subtitle" xml:lang="de">
                        <text for="L2">Second language</text>
                    </translation>
                </translations>
                <transliterations>
                    <transliteration xml:lang="ja-Latn">
                        <text for="L2"><span begin="5.0s" end="5.5s">ta</span><span begin="5.5s" end="6.0s">da </span><span begin="6.0s" end="7.0s">hitori</span><span ttm:role="x-bg"><span begin="7.0s" end="8.0s">himitsu</span></span></text>
                    </transliteration>
                </transliterations>
            </iTunesMetadata>
        </metadata>
    </head>
    <body dur="00:15.500">
        <div begin="00:01.000" end="00:15.000" itunes:song-part="Verse">
            <p begin="00:01.000" end="00:04.000" itunes:key="L1" ttm:agent="v1">
                <span begin="00:01.000" end="00:01.800">君</span>
                <span begin="00:01.900" end="00:02.100">の</span>
                <span begin="00:02.200" end="00:03.000">Gold </span>
                <span begin="00:03.000" end="00:03.500">bright</span>
                <span ttm:role="x-translation" xml:lang="en">Inline translation</span>
                <span ttm:role="x-translation" xml:lang="zh-CN">second</span>
                <span ttm:role="x-roman" xml:lang="ja-Latn">kimi no</span>
                <span ttm:role="x-bg" begin="00:03.500" end="00:04.000">
                    <span begin="00:03.500" end="00:04.000">秘密</span>
                    <span ttm:role="x-translation" xml:lang="en">a secret</span>
                    <span ttm:role="x-roman" xml:lang="ja-Latn">himitsu</span>
                </span>
            </p>
            <p begin="5.0" end="8.0" itunes:key="L2" ttm:agent="v1"><span begin="5.0" end="5.5">た</span><span begin="5.5" end="6.0">だ</span> <span begin="6.0" end="7.0">一人</span><span ttm:role="x-bg"><span begin="7.0" end="8.0">ひみつ</span></span><span begin="7.0" end="7.0"/></p>
        </div>
    </body>
</tt>"#;

    #[test]
    fn follows_the_amll_spec() {
        let l = parse(AMLL, Credits::default()).unwrap();
        assert_eq!(l.kind, SyncKind::Syllable);
        assert_eq!(l.lines.len(), 2);
        let first = &l.lines[0];
        assert_eq!(first.lead.text(), "君のGold bright");
        assert!(first.lead.syllables[0].part_of_word);
        assert_eq!(first.lead.translation.as_deref(), Some("Inline translation"));
        assert_eq!(first.lead.roman.as_deref(), Some("kimi no"));
        assert_eq!(first.background.len(), 1);
        assert_eq!(first.background[0].text(), "秘密");
        assert_eq!(first.background[0].translation.as_deref(), Some("a secret"));
        assert_eq!(first.background[0].roman.as_deref(), Some("himitsu"));
        let second = &l.lines[1];
        assert_eq!(second.lead.text(), "ただ 一人");
        assert_eq!(second.lead.translation.as_deref(), Some("Head translation"));
        let romans: Vec<&str> = second.lead.syllables.iter().filter_map(|s| s.roman.as_deref()).collect();
        assert_eq!(romans, ["ta", "da", "hitori"]);
        assert!(second.lead.syllables[0].roman_part_of_word);
        assert!(!second.lead.syllables[1].roman_part_of_word);
        assert_eq!(second.background[0].translation.as_deref(), Some("bg translation"));
        assert_eq!(second.background[0].syllables[0].roman.as_deref(), Some("himitsu"));
        assert_eq!(l.credits.contributors.len(), 1);
        assert_eq!(l.credits.contributors[0].url.as_deref(), Some("https://github.com/someone"));
        let meta = metadata(AMLL);
        assert_eq!(meta.titles, vec!["Song Title".to_string()]);
        assert_eq!(meta.artists, vec!["Artist A".to_string()]);
        assert_eq!(meta.spotify_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn parses_line_timing_with_aux_spans() {
        let doc = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xmlns:itunes="x" itunes:timing="Line"><body><div><p begin="1.0" end="3.0" itunes:key="L1">A line of lyrics<span ttm:role="x-translation" xml:lang="de">Eine Zeile</span><span ttm:role="x-bg">echo</span></p><p begin="4.0" end="5.0"><span ttm:role="x-bg">only bg</span></p></div></body></tt>"#;
        let l = parse(doc, Credits::default()).unwrap();
        assert_eq!(l.kind, SyncKind::Line);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].lead.text(), "A line of lyrics");
        assert_eq!(l.lines[0].lead.translation.as_deref(), Some("Eine Zeile"));
        assert_eq!(l.lines[0].background[0].text(), "echo");
        assert!(l.lines[1].background_only);
        assert_eq!(l.lines[1].lead.text(), "only bg");
    }

    #[test]
    fn strips_parentheses_around_background_vocals() {
        let doc = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" itunes:timing="Word"><head><metadata><ttm:agent type="person" xml:id="v1"/></metadata></head><body><div><p begin="00:14.300" end="00:17.795" ttm:agent="v1" itunes:key="L1"><span begin="00:14.300" end="00:14.500">Cribs</span> <span begin="00:14.500" end="00:16.000">overseas</span><span ttm:role="x-bg" begin="00:16.885" end="00:17.795"><span begin="00:16.885" end="00:17.020">(For</span> <span begin="00:17.020" end="00:17.175">cribs</span> <span begin="00:17.175" end="00:17.315">o</span><span begin="00:17.315" end="00:17.461">ver</span><span begin="00:17.461" end="00:17.795">seas)</span></span></p><p begin="01:35.000" end="01:36.000" ttm:agent="v1" itunes:key="L2"><span begin="01:35.000" end="01:35.500">Yeah</span><span ttm:role="x-bg"><span begin="01:35.532" end="01:35.744">(Oh)</span> <span begin="01:35.800" end="01:35.900">(yeah)</span></span></p><p begin="02:00.000" end="02:01.000" ttm:agent="v1" itunes:key="L3"><span begin="02:00.000" end="02:00.500">Go</span><span ttm:role="x-bg"><span begin="02:00.500" end="02:00.600">(</span><span begin="02:00.600" end="02:01.000">go)</span></span></p></div></body></tt>"#;
        let l = parse(doc, Credits::default()).unwrap();
        assert_eq!(l.lines[0].lead.text(), "Cribs overseas");
        assert_eq!(l.lines[0].background[0].text(), "For cribs overseas");
        assert_eq!(l.lines[0].background[0].syllables.len(), 5);
        assert_eq!(l.lines[1].background[0].text(), "(Oh) (yeah)");
        assert_eq!(l.lines[2].background[0].text(), "go");
        assert_eq!(l.lines[2].background[0].syllables.len(), 1);
    }

    #[test]
    fn finds_wrapping_parentheses() {
        assert_eq!(paren_edges("(For cribs overseas)"), (true, true));
        assert_eq!(paren_edges("(a (b) c)"), (true, true));
        assert_eq!(paren_edges("(Oh) (yeah)"), (false, false));
        assert_eq!(paren_edges("（ひみつ）"), (true, true));
        assert_eq!(paren_edges("(still going"), (true, false));
        assert_eq!(paren_edges("ending)"), (false, true));
        assert_eq!(paren_edges("plain"), (false, false));
    }
}
