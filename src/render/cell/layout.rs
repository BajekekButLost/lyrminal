use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::lyrics::process::{ItemKind, Prepared};
use crate::lyrics::{Credits, SyncKind, VocalGroup};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Lead,
    Bg(usize),
}

#[derive(Debug, Clone)]
pub struct Glyph {
    pub text: String,
    pub width: u16,
    pub col: u16,
    pub syl: usize,
    pub index_in_syl: usize,
    pub syl_len: usize,
    pub word: usize,
    pub letter: usize,
    pub word_len: usize,
    pub space: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowKind {
    Lyric(Group),
    Roman { group: Group, timed: bool },
    Translation,
    Interlude,
    Credit { emphasis: bool },
    Notice,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub kind: RowKind,
    pub glyphs: Vec<Glyph>,
    pub width: u16,
    pub align: Align,
    pub row_in_group: usize,
    pub rows_in_group: usize,
    pub link: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub item: Option<usize>,
    pub top: usize,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub blocks: Vec<Block>,
    pub total: usize,
    pub width: u16,
    pub item_tops: Vec<f64>,
    pub item_blocks: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutOptions {
    pub width: u16,
    pub romanization: bool,
    pub translation: bool,
    pub background: bool,
    pub gap: u16,
    pub center: bool,
}

struct Token {
    glyphs: Vec<Glyph>,
    width: u16,
}

fn tokens(group: &VocalGroup, roman: bool) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut current: Vec<Glyph> = Vec::new();
    let mut word = 0usize;
    let flush = |current: &mut Vec<Glyph>, out: &mut Vec<Token>, word: &mut usize| {
        if current.is_empty() {
            return;
        }
        let len = current.len();
        for (i, g) in current.iter_mut().enumerate() {
            g.letter = i;
            g.word_len = len;
            g.word = *word;
        }
        let width = current.iter().map(|g| g.width).sum();
        out.push(Token {
            glyphs: std::mem::take(current),
            width,
        });
        *word += 1;
    };
    for (si, s) in group.syllables.iter().enumerate() {
        let (text, part_of_word) = if roman {
            match &s.roman {
                Some(r) => (r.as_str(), s.roman_part_of_word),
                None => (s.text.as_str(), s.part_of_word),
            }
        } else {
            (s.text.as_str(), s.part_of_word)
        };
        let graphemes: Vec<&str> = text.graphemes(true).collect();
        let syl_len = graphemes.len().max(1);
        for (gi, g) in graphemes.iter().enumerate() {
            if g.chars().all(char::is_whitespace) {
                flush(&mut current, &mut out, &mut word);
                continue;
            }
            current.push(Glyph {
                text: g.to_string(),
                width: UnicodeWidthStr::width(*g).max(1) as u16,
                col: 0,
                syl: si,
                index_in_syl: gi,
                syl_len,
                word: 0,
                letter: 0,
                word_len: 0,
                space: false,
            });
        }
        if !part_of_word {
            flush(&mut current, &mut out, &mut word);
        }
    }
    flush(&mut current, &mut out, &mut word);
    out
}

fn wrap(tokens: Vec<Token>, width: u16) -> Vec<Vec<Glyph>> {
    let width = width.max(4);
    let mut rows: Vec<Vec<Glyph>> = Vec::new();
    let mut row: Vec<Glyph> = Vec::new();
    let mut col: u16 = 0;
    for tok in tokens {
        if !row.is_empty() && col + 1 + tok.width > width {
            rows.push(std::mem::take(&mut row));
            col = 0;
        }
        if !row.is_empty() {
            let prev = row.last().unwrap();
            row.push(Glyph {
                text: " ".into(),
                width: 1,
                col,
                syl: prev.syl,
                index_in_syl: prev.index_in_syl,
                syl_len: prev.syl_len,
                word: prev.word,
                letter: prev.letter,
                word_len: prev.word_len,
                space: true,
            });
            col += 1;
        }
        for mut g in tok.glyphs {
            if col + g.width > width && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                col = 0;
            }
            g.col = col;
            col += g.width;
            row.push(g);
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

fn plain_tokens(text: &str) -> Vec<Token> {
    let group = VocalGroup::single(text.to_string(), 0.0, 0.0);
    tokens(&group, false)
}

fn rows_from(glyph_rows: Vec<Vec<Glyph>>, kind: RowKind, align: Align, link: Option<String>) -> Vec<Row> {
    let n = glyph_rows.len();
    glyph_rows
        .into_iter()
        .enumerate()
        .map(|(i, glyphs)| Row {
            width: glyphs.last().map(|g| g.col + g.width).unwrap_or(0),
            kind: kind.clone(),
            glyphs,
            align,
            row_in_group: i,
            rows_in_group: n,
            link: link.clone(),
        })
        .collect()
}

pub fn text_rows(text: &str, width: u16, kind: RowKind, align: Align, link: Option<String>) -> Vec<Row> {
    rows_from(wrap(plain_tokens(text), width), kind, align, link)
}

fn credit_rows(credits: &Credits, width: u16, align: Align) -> Vec<Row> {
    let mut rows = Vec::new();
    if !credits.songwriters.is_empty() {
        rows.extend(text_rows(
            &format!("Written by {}", credits.songwriters.join(", ")),
            width,
            RowKind::Credit { emphasis: false },
            align,
            None,
        ));
    }
    let mut provided = format!("Lyrics provided by {}", credits.provider);
    if let Some(src) = &credits.source {
        provided.push_str(&format!(" · {src}"));
    }
    rows.extend(text_rows(
        &provided,
        width,
        RowKind::Credit { emphasis: true },
        align,
        credits.provider_url.clone(),
    ));
    for c in &credits.contributors {
        rows.extend(text_rows(
            &format!("{} {}", c.role, c.name),
            width,
            RowKind::Credit { emphasis: true },
            align,
            c.url.clone(),
        ));
    }
    rows
}

pub fn build(prepared: &Prepared, opts: LayoutOptions) -> Layout {
    let width = opts.width.max(8);
    let has_duets = prepared.lyrics.lines.iter().any(|l| l.opposite);
    let inset = if has_duets { (width as f32 * 0.15) as u16 } else { 0 };
    let lyric_width = width.saturating_sub(inset).max(8);
    let kind = prepared.lyrics.kind;
    let mut blocks: Vec<Block> = Vec::new();
    let mut top = 0usize;
    let mut item_tops = Vec::with_capacity(prepared.items.len());
    let mut item_blocks = Vec::with_capacity(prepared.items.len());
    let gap = opts.gap as usize;

    for (idx, item) in prepared.items.iter().enumerate() {
        let align = if opts.center {
            Align::Center
        } else if item.opposite {
            Align::Right
        } else {
            Align::Left
        };
        let mut rows: Vec<Row> = Vec::new();
        match item.kind {
            ItemKind::Interlude { .. } => rows.push(Row {
                kind: RowKind::Interlude,
                glyphs: Vec::new(),
                width: 5,
                align,
                row_in_group: 0,
                rows_in_group: 1,
                link: None,
            }),
            ItemKind::Line(li) => {
                let line = &prepared.lyrics.lines[li];
                rows.extend(rows_from(
                    wrap(tokens(&line.lead, false), lyric_width),
                    RowKind::Lyric(Group::Lead),
                    align,
                    None,
                ));
                if opts.romanization {
                    let per_syllable = line.lead.syllables.iter().any(|s| s.roman.is_some());
                    if per_syllable && kind == SyncKind::Syllable {
                        rows.extend(rows_from(
                            wrap(tokens(&line.lead, true), lyric_width),
                            RowKind::Roman {
                                group: Group::Lead,
                                timed: true,
                            },
                            align,
                            None,
                        ));
                    } else if let Some(r) = line.lead.roman_text() {
                        rows.extend(text_rows(
                            &r,
                            lyric_width,
                            RowKind::Roman {
                                group: Group::Lead,
                                timed: false,
                            },
                            align,
                            None,
                        ));
                    }
                }
                if opts.background {
                    for (bi, bg) in line.background.iter().enumerate() {
                        rows.extend(rows_from(
                            wrap(tokens(bg, false), lyric_width),
                            RowKind::Lyric(Group::Bg(bi)),
                            align,
                            None,
                        ));
                        if opts.romanization && bg.syllables.iter().any(|s| s.roman.is_some()) {
                            rows.extend(rows_from(
                                wrap(tokens(bg, true), lyric_width),
                                RowKind::Roman {
                                    group: Group::Bg(bi),
                                    timed: true,
                                },
                                align,
                                None,
                            ));
                        }
                    }
                }
                if opts.translation
                    && let Some(tr) = &line.lead.translation
                {
                    rows.extend(text_rows(tr, lyric_width, RowKind::Translation, align, None));
                }
            }
        }
        if rows.is_empty() {
            rows.push(Row {
                kind: RowKind::Lyric(Group::Lead),
                glyphs: Vec::new(),
                width: 0,
                align,
                row_in_group: 0,
                rows_in_group: 1,
                link: None,
            });
        }
        item_tops.push(top as f64);
        item_blocks.push(blocks.len());
        let height = rows.len();
        blocks.push(Block {
            item: Some(idx),
            top,
            rows,
        });
        top += height + gap;
    }

    if prepared.lyrics.instrumental {
        let rows = text_rows("♪ Instrumental", width, RowKind::Notice, Align::Center, None);
        let h = rows.len();
        blocks.push(Block { item: None, top, rows });
        top += h + gap;
    }

    let credit_align = if opts.center { Align::Center } else { Align::Left };
    let credits = credit_rows(&prepared.lyrics.credits, width, credit_align);
    let h = credits.len();
    top += 1;
    blocks.push(Block {
        item: None,
        top,
        rows: credits,
    });
    top += h;

    Layout {
        blocks,
        total: top,
        width,
        item_tops,
        item_blocks,
    }
}

pub fn row_x(row: &Row, width: u16) -> u16 {
    match row.align {
        Align::Left => 0,
        Align::Right => width.saturating_sub(row.width),
        Align::Center => width.saturating_sub(row.width) / 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::process::{PrepareOptions, prepare};
    use crate::lyrics::{LyricLine, Lyrics, Syllable};

    fn syl(text: &str, part: bool) -> Syllable {
        Syllable {
            text: text.into(),
            start: 0.0,
            end: 1.0,
            part_of_word: part,
            roman: None,
            roman_part_of_word: false,
        }
    }

    #[test]
    fn wraps_words_without_splitting_syllables() {
        let group = VocalGroup {
            start: 0.0,
            end: 1.0,
            syllables: vec![syl("won", true), syl("der", false), syl("ful", false), syl("world", false)],
            roman: None,
            translation: None,
        };
        let rows = wrap(tokens(&group, false), 10);
        let texts: Vec<String> = rows
            .iter()
            .map(|r| r.iter().map(|g| g.text.as_str()).collect::<String>())
            .collect();
        assert_eq!(texts, vec!["wonder ful", "world"]);
        assert_eq!(rows[0][3].syl, 1);
        assert_eq!(rows[0][3].index_in_syl, 0);
        assert_eq!(rows[0][3].letter, 3);
        assert_eq!(rows[0][3].word_len, 6);
    }

    #[test]
    fn builds_blocks_with_credits() {
        let lyrics = Lyrics {
            kind: SyncKind::Line,
            lines: vec![LyricLine {
                start: 0.0,
                end: 1.0,
                opposite: true,
                lead: VocalGroup::single("hello there".into(), 0.0, 1.0),
                background: vec![],
            }],
            credits: Credits {
                provider: "X".into(),
                ..Default::default()
            },
            language: None,
            instrumental: false,
        };
        let p = prepare(
            &lyrics,
            PrepareOptions {
                interludes: true,
                interlude_threshold: 3.0,
                extend_lines: false,
                join_split_words: false,
            },
        );
        let l = build(
            &p,
            LayoutOptions {
                width: 40,
                romanization: false,
                translation: false,
                background: true,
                gap: 1,
                center: false,
            },
        );
        assert_eq!(l.blocks[0].rows[0].align, Align::Right);
        assert!(l.blocks.last().unwrap().rows.iter().any(|r| r.link.is_none()));
        assert_eq!(l.item_tops.len(), 1);
    }
}
