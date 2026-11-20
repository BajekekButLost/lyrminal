pub mod lrc;
pub mod process;
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
    #[cfg(test)]
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
}

impl LyricLine {
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
    pub source: Option<String>,
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
        if self.kind != SyncKind::Static {
            self.lines
                .sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        }
    }
}
