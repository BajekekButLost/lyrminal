pub mod ass;
pub mod background;
pub mod cue;
pub mod formats;
pub mod json;
pub mod lrc;
pub mod process;
pub mod sami;
pub mod subtitles;
pub mod ttml;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncKind {
    Syllable,
    Line,
    Static,
}

impl SyncKind {
    pub fn label(self) -> &'static str {
        match self {
            SyncKind::Syllable => "Word sync",
            SyncKind::Line => "Line sync",
            SyncKind::Static => "Unsynced",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            SyncKind::Syllable => 2,
            SyncKind::Line => 1,
            SyncKind::Static => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Syllable {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub part_of_word: bool,
    #[serde(default)]
    pub roman: Option<String>,
    #[serde(default)]
    pub roman_part_of_word: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VocalGroup {
    pub start: f64,
    pub end: f64,
    pub syllables: Vec<Syllable>,
    #[serde(default)]
    pub roman: Option<String>,
    #[serde(default)]
    pub translation: Option<String>,
}

impl VocalGroup {
    pub fn text(&self) -> String {
        join_syllables(self.syllables.iter().map(|s| (s.text.as_str(), s.part_of_word)))
    }

    pub fn roman_text(&self) -> Option<String> {
        if self.syllables.iter().any(|s| s.roman.is_some()) {
            Some(join_syllables(self.syllables.iter().map(|s| {
                (
                    s.roman.as_deref().unwrap_or(s.text.as_str()),
                    if s.roman.is_some() {
                        s.roman_part_of_word
                    } else {
                        s.part_of_word
                    },
                )
            })))
        } else {
            self.roman.clone()
        }
    }

    pub fn keeps_syllables_whole(&self, kind: SyncKind) -> bool {
        kind == SyncKind::Syllable && self.syllables.len() > 1
    }

    pub fn single(text: String, start: f64, end: f64) -> Self {
        Self {
            start,
            end,
            syllables: vec![Syllable {
                text,
                start,
                end,
                part_of_word: false,
                roman: None,
                roman_part_of_word: false,
            }],
            roman: None,
            translation: None,
        }
    }
}

fn join_syllables<'a>(parts: impl Iterator<Item = (&'a str, bool)>) -> String {
    let mut out = String::new();
    let mut glue = true;
    for (text, part_of_word) in parts {
        if !glue && !out.is_empty() {
            out.push(' ');
        }
        out.push_str(text);
        glue = part_of_word;
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricLine {
    pub start: f64,
    pub end: f64,
    pub opposite: bool,
    pub lead: VocalGroup,
    #[serde(default)]
    pub background: Vec<VocalGroup>,
    #[serde(default)]
    pub background_only: bool,
}

impl LyricLine {
    pub fn normalize(&mut self, timed: bool) {
        self.background.retain(|g| !g.syllables.is_empty());
        if self.lead.syllables.is_empty() && !self.background.is_empty() {
            let old = std::mem::replace(&mut self.lead, self.background.remove(0));
            self.lead.roman = self.lead.roman.take().or(old.roman);
            self.lead.translation = self.lead.translation.take().or(old.translation);
            self.background_only = true;
        }
        if !timed {
            return;
        }
        for g in std::iter::once(&mut self.lead).chain(self.background.iter_mut()) {
            if let (Some(first), Some(last)) = (g.syllables.first(), g.syllables.last()) {
                g.start = g.start.min(first.start);
                g.end = g.end.max(last.end);
            }
        }
        let start = std::iter::once(&self.lead)
            .chain(self.background.iter())
            .filter(|g| !g.syllables.is_empty())
            .map(|g| g.start)
            .fold(f64::MAX, f64::min);
        if start < f64::MAX {
            self.start = if self.background_only { start } else { self.start.min(start) };
        }
        self.end = self.end.max(self.lead.end).max(self.start);
    }

    pub fn full_end(&self) -> f64 {
        self.background
            .iter()
            .map(|b| b.end)
            .fold(self.end.max(self.lead.end), f64::max)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contributor {
    pub role: String,
    pub name: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Credits {
    pub provider: String,
    #[serde(default)]
    pub provider_url: Option<String>,
    #[serde(default)]
    pub provider_credit: Option<String>,
    pub source: Option<String>,
    #[serde(default)]
    pub source_tag: Option<String>,
    pub songwriters: Vec<String>,
    pub contributors: Vec<Contributor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lyrics {
    pub kind: SyncKind,
    pub lines: Vec<LyricLine>,
    pub credits: Credits,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub instrumental: bool,
}

impl Lyrics {
    pub fn has_romanization(&self) -> bool {
        self.lines.iter().any(|l| {
            l.lead.roman.is_some()
                || l.lead.syllables.iter().any(|s| s.roman.is_some())
                || l.background.iter().any(|b| b.roman.is_some())
        })
    }

    pub fn has_translation(&self) -> bool {
        self.lines.iter().any(|l| l.lead.translation.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn sort(&mut self) {
        let timed = self.kind != SyncKind::Static;
        for line in &mut self.lines {
            line.normalize(timed);
        }
        self.lines.retain(|l| !l.lead.syllables.is_empty());
        if timed {
            self.place_out_of_sequence_lines();
        }
    }

    fn place_out_of_sequence_lines(&mut self) {
        for i in 1..self.lines.len() {
            let prev_start = self.lines[i - 1].start;
            let prev_end = self.lines[i - 1].full_end();
            let line = &mut self.lines[i];
            if line.start < prev_start && line.full_end() <= prev_start {
                line.shift(prev_end - line.start);
            }
        }
    }
}

impl LyricLine {
    fn shift(&mut self, by: f64) {
        self.start += by;
        self.end += by;
        for g in std::iter::once(&mut self.lead).chain(self.background.iter_mut()) {
            g.start += by;
            g.end += by;
            for s in &mut g.syllables {
                s.start += by;
                s.end += by;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start: f64, end: f64, text: &str) -> LyricLine {
        LyricLine {
            start,
            end,
            opposite: false,
            lead: VocalGroup::single(text.into(), start, end),
            background: vec![],
            background_only: false,
        }
    }

    #[test]
    fn keeps_the_order_of_the_sync() {
        let mut l = Lyrics {
            kind: SyncKind::Syllable,
            lines: vec![
                line(5.0, 7.0, "a"),
                line(6.5, 9.0, "b"),
                line(10.0, 12.0, "c"),
                line(0.0, 0.0, "d"),
                line(0.0, 0.0, "e"),
            ],
            credits: Credits::default(),
            language: None,
            instrumental: false,
        };
        l.sort();
        let texts: Vec<String> = l.lines.iter().map(|l| l.lead.text()).collect();
        assert_eq!(texts, ["a", "b", "c", "d", "e"]);
        assert_eq!((l.lines[1].start, l.lines[1].end), (6.5, 9.0));
        assert_eq!((l.lines[3].start, l.lines[3].end), (12.0, 12.0));
        assert_eq!(l.lines[3].lead.syllables[0].start, 12.0);
        assert_eq!((l.lines[4].start, l.lines[4].end), (12.0, 12.0));
    }
}
