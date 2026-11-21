use std::ops::Range;

use super::{background, Lyrics, Syllable, SyncKind, VocalGroup};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    NotSung,
    Active,
    Sung,
}

pub fn state(t: f64, start: f64, end: f64) -> State {
    if t < start {
        State::NotSung
    } else if t >= end {
        State::Sung
    } else {
        State::Active
    }
}

pub fn progress(t: f64, start: f64, end: f64) -> f64 {
    if end <= start {
        return if t >= start { 1.0 } else { 0.0 };
    }
    ((t - start) / (end - start)).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ItemKind {
    Line(usize),
    Interlude { start: f64, end: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub start: f64,
    pub end: f64,
    pub opposite: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PrepareOptions {
    pub interludes: bool,
    pub interlude_threshold: f64,
    pub extend_lines: bool,
    pub join_split_words: bool,
    pub fix_background: bool,
}

#[derive(Debug, Clone)]
pub struct Prepared {
    pub lyrics: Lyrics,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct WordSpan {
    pub syllables: Range<usize>,
    pub start: f64,
    pub end: f64,
    pub letters: usize,
}

pub fn words(group: &VocalGroup) -> Vec<WordSpan> {
    let mut out = Vec::new();
    let mut begin = 0;
    for (i, s) in group.syllables.iter().enumerate() {
        if !s.part_of_word || i + 1 == group.syllables.len() {
            let range = begin..i + 1;
            let syl = &group.syllables[range.clone()];
            out.push(WordSpan {
                start: syl.first().map(|s| s.start).unwrap_or(0.0),
                end: syl.last().map(|s| s.end).unwrap_or(0.0),
                letters: syl.iter().map(|s| letter_count(&s.text)).sum(),
                syllables: range,
            });
            begin = i + 1;
        }
    }
    out
}

pub fn letter_count(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

pub fn is_rtl(text: &str) -> bool {
    text.chars().any(|c| {
        let c = c as u32;
        (0x0590..=0x08FF).contains(&c) || (0xFB1D..=0xFDFF).contains(&c) || (0xFE70..=0xFEFF).contains(&c)
    })
}

pub fn is_unspaced(text: &str) -> bool {
    text.chars().any(|c| {
        let c = c as u32;
        (0x3040..=0x30FF).contains(&c)
            || (0x3400..=0x9FFF).contains(&c)
            || (0xAC00..=0xD7AF).contains(&c)
            || (0x0E00..=0x0E7F).contains(&c)
            || (0xF900..=0xFAFF).contains(&c)
    })
}

pub fn spring_emphasis(s: &Syllable) -> bool {
    s.end - s.start >= 1.0 && letter_count(&s.text) >= 1 && !is_rtl(&s.text)
}

pub fn bloom_emphasis(group: &VocalGroup, word: &WordSpan, background: bool) -> bool {
    if background || word.end - word.start < 1.0 || !(2..=7).contains(&word.letters) {
        return false;
    }
    !group.syllables[word.syllables.clone()]
        .iter()
        .any(|s| is_rtl(&s.text))
}

fn join_group(group: &mut VocalGroup) {
    let mut out: Vec<Syllable> = Vec::new();
    let mut run: Vec<Syllable> = Vec::new();
    let flush = |run: &mut Vec<Syllable>, out: &mut Vec<Syllable>| {
        if run.is_empty() {
            return;
        }
        let blocked = run.len() == 1
            || run.iter().any(|s| {
                is_unspaced(&s.text) || s.text.contains(' ') || s.text.ends_with('-') || s.text.ends_with('/')
            });
        if blocked {
            out.append(run);
            return;
        }
        let first = run.first().cloned().unwrap();
        let last = run.last().cloned().unwrap();
        let roman = if run.iter().all(|s| s.roman.is_some()) {
            Some(run.iter().filter_map(|s| s.roman.clone()).collect::<String>())
        } else {
            None
        };
        out.push(Syllable {
            text: run.iter().map(|s| s.text.as_str()).collect(),
            start: first.start,
            end: last.end,
            part_of_word: last.part_of_word,
            roman,
            roman_part_of_word: last.roman_part_of_word,
        });
        run.clear();
    };
    for s in group.syllables.drain(..) {
        let continues = s.part_of_word;
        run.push(s);
        if !continues {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    group.syllables = out;
}

pub fn prepare(source: &Lyrics, opts: PrepareOptions) -> Prepared {
    let mut lyrics = source.clone();
    if opts.fix_background {
        background::split_background(&mut lyrics);
    }
    lyrics.sort();
    if opts.join_split_words && lyrics.kind == SyncKind::Syllable {
        for line in &mut lyrics.lines {
            join_group(&mut line.lead);
            for bg in &mut line.background {
                join_group(bg);
            }
        }
    }
    let mut items = Vec::new();
    if lyrics.kind == SyncKind::Static {
        for (i, _) in lyrics.lines.iter().enumerate() {
            items.push(Item {
                kind: ItemKind::Line(i),
                start: 0.0,
                end: 0.0,
                opposite: false,
            });
        }
        return Prepared { lyrics, items };
    }
    let n = lyrics.lines.len();
    if opts.extend_lines {
        for i in 0..n.saturating_sub(1) {
            let next_start = lyrics.lines[i + 1].start;
            let line = &mut lyrics.lines[i];
            let gap = next_start - line.end;
            if gap > 0.0 && gap < opts.interlude_threshold {
                line.end = next_start;
            }
        }
    }
    let threshold = opts.interlude_threshold;
    for i in 0..n {
        let line = &lyrics.lines[i];
        if opts.interludes {
            let prev_end = if i == 0 { 0.0 } else { lyrics.lines[i - 1].end };
            if line.start - prev_end >= threshold {
                items.push(Item {
                    kind: ItemKind::Interlude {
                        start: prev_end,
                        end: line.start,
                    },
                    start: prev_end,
                    end: line.start,
                    opposite: line.opposite,
                });
            }
        }
        items.push(Item {
            kind: ItemKind::Line(i),
            start: line.start,
            end: line.full_end(),
            opposite: line.opposite,
        });
    }
    Prepared { lyrics, items }
}

impl Prepared {
    pub fn active_items(&self, t: f64) -> impl Iterator<Item = usize> + '_ {
        self.items
            .iter()
            .enumerate()
            .filter(move |(_, it)| t >= it.start && t < it.end)
            .map(|(i, _)| i)
    }

    pub fn scroll_target(&self, t: f64) -> usize {
        if self.items.is_empty() {
            return 0;
        }
        if self.lyrics.kind == SyncKind::Static {
            return 0;
        }
        let active: Vec<usize> = self.active_items(t).collect();
        match active.as_slice() {
            [] => self
                .items
                .iter()
                .rposition(|it| it.start <= t)
                .unwrap_or(0),
            [one] => *one,
            [first, .., last] => {
                let first_end = self.items[*first].end;
                let lookahead = self.items.get(first + 2).map(|it| it.start).unwrap_or(f64::MAX);
                if first_end <= lookahead { *first } else { *last }
            }
        }
    }

    pub fn all_sung(&self, t: f64) -> bool {
        self.items.last().map(|it| t >= it.end).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::{Credits, LyricLine};

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

    fn opts() -> PrepareOptions {
        PrepareOptions {
            interludes: true,
            interlude_threshold: 3.0,
            extend_lines: false,
            join_split_words: false,
            fix_background: false,
        }
    }

    #[test]
    fn inserts_interludes() {
        let l = Lyrics {
            kind: SyncKind::Line,
            lines: vec![line(5.0, 7.0, "a"), line(7.5, 8.0, "b"), line(15.0, 16.0, "c")],
            credits: Credits::default(),
            language: None,
            instrumental: false,
        };
        let p = prepare(&l, opts());
        assert_eq!(p.items.len(), 5);
        assert!(matches!(p.items[0].kind, ItemKind::Interlude { .. }));
        assert!(matches!(p.items[3].kind, ItemKind::Interlude { .. }));
        assert_eq!(p.scroll_target(7.2), 1);
        assert_eq!(p.scroll_target(10.0), 3);
    }

    #[test]
    fn interlude_ends_when_opening_background_starts() {
        let mut first = line(6.0, 7.0, "lead");
        first.background.push(VocalGroup::single("(intro)".into(), 4.0, 5.5));
        let l = Lyrics {
            kind: SyncKind::Syllable,
            lines: vec![first],
            credits: Credits::default(),
            language: None,
            instrumental: false,
        };
        let p = prepare(&l, opts());
        assert_eq!(p.items.len(), 2);
        assert!((p.items[0].end - 4.0).abs() < 1e-9);
        assert!((p.items[1].start - 4.0).abs() < 1e-9);
        assert_eq!(p.scroll_target(4.5), 1);
    }

    #[test]
    fn groups_words() {
        let mut g = VocalGroup::single("al".into(), 0.0, 1.0);
        g.syllables[0].part_of_word = true;
        g.syllables.push(Syllable {
            text: "pha".into(),
            start: 1.0,
            end: 2.0,
            part_of_word: false,
            roman: None,
            roman_part_of_word: false,
        });
        let w = words(&g);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].letters, 5);
        join_group(&mut g);
        assert_eq!(g.syllables.len(), 1);
        assert_eq!(g.syllables[0].text, "alpha");
    }
}
