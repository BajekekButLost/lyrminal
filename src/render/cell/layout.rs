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
    pub links: Vec<RowLink>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RowLink {
    pub start: u16,
    pub end: u16,
    pub url: String,
}

impl Row {
    pub fn link_at(&self, col: u16) -> Option<usize> {
        self.links.iter().position(|l| col >= l.start && col < l.end)
    }
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
    pub item_tops: Vec<f64>,
    pub items_end: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutOptions {
    pub width: u16,
    pub romanization: bool,
    pub translation: bool,
    pub background: bool,
    pub gap: u16,
    pub center: bool,
    pub header: bool,
}

struct Token {
    glyphs: Vec<Glyph>,
    width: u16,
}

fn tokens(group: &VocalGroup, roman: bool, glue: bool) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut current: Vec<Glyph> = Vec::new();
    let mut word = 0usize;
    let flush = |current: &mut Vec<Glyph>, out: &mut Vec<Token>, word: &mut usize| {
        if current.is_empty() {
            return;
        }
        let len = current.iter().filter(|g| !g.space).count();
        let mut letter = 0usize;
        for g in current.iter_mut() {
            g.letter = letter;
            g.word_len = len;
            g.word = *word;
            if !g.space {
                letter += 1;
            }
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
        let blank = |g: &&str| g.chars().all(char::is_whitespace);
        let first = graphemes.iter().position(|g| !blank(g));
        let last = graphemes.iter().rposition(|g| !blank(g));
        for (gi, g) in graphemes.iter().enumerate() {
            let space = blank(g);
            if space {
                let inner = glue && first.is_some_and(|f| gi > f) && last.is_some_and(|l| gi < l);
                if !inner {
                    flush(&mut current, &mut out, &mut word);
                    continue;
                }
                if current.last().is_some_and(|p| p.space) {
                    continue;
                }
            }
            current.push(Glyph {
                text: if space { " ".into() } else { g.to_string() },
                width: if space { 1 } else { UnicodeWidthStr::width(*g).max(1) as u16 },
                col: 0,
                syl: si,
                index_in_syl: gi,
                syl_len,
                word: 0,
                letter: 0,
                word_len: 0,
                space,
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
            if g.space && row.is_empty() {
                continue;
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
    tokens(&group, false, false)
}

fn rows_from(glyph_rows: Vec<Vec<Glyph>>, kind: RowKind, align: Align, link: Option<String>) -> Vec<Row> {
    glyph_rows
        .into_iter()
        .map(|glyphs| {
            let width = glyphs.last().map(|g| g.col + g.width).unwrap_or(0);
            Row {
                width,
                kind: kind.clone(),
                glyphs,
                align,
                links: link
                    .iter()
                    .map(|url| RowLink {
                        start: 0,
                        end: width,
                        url: url.clone(),
                    })
                    .collect(),
            }
        })
        .collect()
}

fn joined_row(segments: &[(String, Option<String>)], kind: RowKind, align: Align) -> Row {
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut links = Vec::new();
    let mut col = 0u16;
    for (i, (text, url)) in segments.iter().enumerate() {
        if i > 0
            && let Some(prev) = glyphs.last().cloned()
        {
            for (s, space) in [(" ", true), ("·", false), (" ", true)] {
                glyphs.push(Glyph {
                    text: s.into(),
                    width: 1,
                    col,
                    space,
                    ..prev.clone()
                });
                col += 1;
            }
        }
        let start = col;
        let words = glyphs.last().map(|g| g.word + 1).unwrap_or(0);
        for row in wrap(plain_tokens(text), u16::MAX / 2) {
            for mut g in row {
                g.col += start;
                g.word += words;
                col = g.col + g.width;
                glyphs.push(g);
            }
        }
        if let Some(url) = url {
            links.push(RowLink {
                start,
                end: col,
                url: url.clone(),
            });
        }
    }
    Row {
        width: col,
        kind,
        glyphs,
        align,
        links,
    }
}

pub fn provider_in_header(credits: &Credits) -> bool {
    credits.provider == crate::providers::spicy::NAME || credits.provider == crate::providers::lrcmux::NAME
}

pub fn text_rows(text: &str, width: u16, kind: RowKind, align: Align, link: Option<String>) -> Vec<Row> {
    rows_from(wrap(plain_tokens(text), width), kind, align, link)
}

pub fn credit_rows(credits: &Credits, width: u16, align: Align, header: bool) -> Vec<Row> {
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
    let mut segments: Vec<(String, Option<String>)> = Vec::new();
    if let Some(credit) = &credits.provider_credit {
        segments.push((credit.clone(), credits.provider_url.clone()));
    } else if !(header && provider_in_header(credits)) {
        let mut short = credits.provider.clone();
        if let Some(src) = &credits.source {
            short.push_str(&format!(" · {src}"));
        }
        let long = format!("Lyrics provided by {short}");
        let provided = if UnicodeWidthStr::width(long.as_str()) <= width as usize { long } else { short };
        rows.extend(text_rows(
            &provided,
            width,
            RowKind::Credit { emphasis: true },
            align,
            credits.provider_url.clone(),
        ));
    }
    segments.extend(
        credits
            .contributors
            .iter()
            .map(|c| (format!("{} {}", c.role, c.name), c.url.clone())),
    );
    if segments.is_empty() {
        return rows;
    }
    let joined = joined_row(&segments, RowKind::Credit { emphasis: true }, align);
    if joined.width <= width {
        rows.push(joined);
    } else {
        for (text, url) in segments {
            rows.extend(text_rows(&text, width, RowKind::Credit { emphasis: true }, align, url));
        }
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
                links: Vec::new(),
            }),
            ItemKind::Line(li) => {
                let line = &prepared.lyrics.lines[li];
                rows.extend(rows_from(
                    wrap(tokens(&line.lead, false, line.lead.keeps_syllables_whole(kind)), lyric_width),
                    RowKind::Lyric(Group::Lead),
                    align,
                    None,
                ));
                if opts.romanization {
                    let per_syllable = line.lead.syllables.iter().any(|s| s.roman.is_some());
                    if per_syllable && kind == SyncKind::Syllable {
                        rows.extend(rows_from(
                            wrap(tokens(&line.lead, true, line.lead.keeps_syllables_whole(kind)), lyric_width),
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
                            wrap(tokens(bg, false, bg.keeps_syllables_whole(kind)), lyric_width),
                            RowKind::Lyric(Group::Bg(bi)),
                            align,
                            None,
                        ));
                        if opts.romanization && bg.syllables.iter().any(|s| s.roman.is_some()) {
                            rows.extend(rows_from(
                                wrap(tokens(bg, true, bg.keeps_syllables_whole(kind)), lyric_width),
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
                links: Vec::new(),
            });
        }
        item_tops.push(top as f64);
        let height = rows.len();
        blocks.push(Block {
            item: Some(idx),
            top,
            rows,
        });
        top += height + gap;
    }
    let items_end = top as f64;

    if prepared.lyrics.instrumental {
        let rows = text_rows("♪ Instrumental", width, RowKind::Notice, Align::Center, None);
        let h = rows.len();
        blocks.push(Block { item: None, top, rows });
        top += h + gap;
    }

    let credit_align = if opts.center { Align::Center } else { Align::Left };
    let credits = credit_rows(&prepared.lyrics.credits, width, credit_align, opts.header);
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
        item_tops,
        items_end,
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
        let rows = wrap(tokens(&group, false, true), 10);
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
    fn keeps_multi_word_syllables_on_one_row() {
        let group = VocalGroup {
            syllables: vec![syl("hold", false), syl(" on  to ", false), syl("me", false)],
            ..Default::default()
        };
        let texts = |glue: bool| -> Vec<String> {
            wrap(tokens(&group, false, glue), 9)
                .iter()
                .map(|r| r.iter().map(|g| g.text.as_str()).collect())
                .collect()
        };
        assert_eq!(texts(false), vec!["hold on", "to me"]);
        assert_eq!(texts(true), vec!["hold", "on to me"]);
        let toks = tokens(&group, false, true);
        assert_eq!(toks.len(), 3);
        let inner = &toks[1].glyphs;
        assert!(inner[2].space);
        assert_eq!((inner[3].letter, inner[3].word_len, inner[3].word), (2, 4, 1));
        assert_eq!(toks[2].glyphs[0].word, 2);
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
                background_only: false,
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
                fix_background: false,
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
                header: true,
            },
        );
        assert_eq!(l.blocks[0].rows[0].align, Align::Right);
        assert!(l.blocks.last().unwrap().rows.iter().any(|r| r.links.is_empty()));
        assert_eq!(l.item_tops.len(), 1);
    }

    fn row_text(row: &Row) -> String {
        row.glyphs.iter().map(|g| g.text.as_str()).collect()
    }

    fn spicy_credits() -> Credits {
        Credits {
            provider: crate::providers::spicy::NAME.into(),
            source: Some("Community sync".into()),
            contributors: vec![
                crate::lyrics::Contributor {
                    role: "Synced by".into(),
                    name: "ana".into(),
                    url: Some("https://x/ana".into()),
                },
                crate::lyrics::Contributor {
                    role: "Uploaded by".into(),
                    name: "bo".into(),
                    url: Some("https://x/bo".into()),
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn spicy_credit_is_one_line_under_the_header() {
        let rows = credit_rows(&spicy_credits(), 60, Align::Left, true);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row_text(row), "Synced by ana · Uploaded by bo");
        assert_eq!(row.links.len(), 2);
        assert_eq!((row.links[0].start, row.links[0].end), (0, 13));
        assert_eq!((row.links[1].start, row.links[1].end), (16, 30));
        assert_eq!(row.link_at(14), None);
        assert_eq!(row.link_at(20), Some(1));
    }

    #[test]
    fn spicy_credit_names_the_provider_without_the_header() {
        let rows = credit_rows(&spicy_credits(), 60, Align::Left, false);
        assert_eq!(rows.len(), 2);
        assert_eq!(row_text(&rows[0]), "Lyrics provided by Spicy Lyrics · Community sync");
        let narrow = credit_rows(&spicy_credits(), 20, Align::Left, true);
        assert_eq!(narrow.len(), 2);
        assert_eq!(row_text(&narrow[1]), "Uploaded by bo");
    }

    #[test]
    fn other_providers_keep_their_credit_line() {
        let credits = Credits {
            provider: "lrc.red".into(),
            provider_url: Some("https://lrc.red".into()),
            ..Default::default()
        };
        let rows = credit_rows(&credits, 60, Align::Left, true);
        assert_eq!(rows.len(), 1);
        assert_eq!(row_text(&rows[0]), "Lyrics provided by lrc.red");
        assert_eq!(rows[0].links[0].url, "https://lrc.red");
    }

    #[test]
    fn lrcmux_source_moves_to_the_header() {
        let credits = Credits {
            provider: crate::providers::lrcmux::NAME.into(),
            provider_url: Some(crate::providers::lrcmux::URL.into()),
            source: Some("Deezer".into()),
            source_tag: Some("Deezer".into()),
            ..Default::default()
        };
        assert!(provider_in_header(&credits));
        assert!(credit_rows(&credits, 60, Align::Left, true).is_empty());
        let rows = credit_rows(&credits, 60, Align::Left, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(row_text(&rows[0]), "Lyrics provided by lrcmux · Deezer");
        assert_eq!(rows[0].links[0].url, crate::providers::lrcmux::URL);
    }

    #[test]
    fn unison_credit_links_the_song_and_the_submitter() {
        let credits = Credits {
            provider: crate::providers::unison::NAME.into(),
            provider_url: Some("https://unison.betterlyrics.org/song/v1".into()),
            provider_credit: Some(crate::providers::unison::CREDIT.into()),
            contributors: vec![crate::lyrics::Contributor {
                role: "submitted by".into(),
                name: "boidu".into(),
                url: Some("https://unison.betterlyrics.org/curator/k1".into()),
            }],
            ..Default::default()
        };
        for header in [true, false] {
            let rows = credit_rows(&credits, 60, Align::Left, header);
            assert_eq!(rows.len(), 1);
            assert_eq!(row_text(&rows[0]), "Lyrics from Unison · submitted by boidu");
            assert_eq!(rows[0].links[0].url, "https://unison.betterlyrics.org/song/v1");
            assert_eq!(rows[0].links[1].url, "https://unison.betterlyrics.org/curator/k1");
        }
        let narrow = credit_rows(&credits, 20, Align::Left, true);
        assert_eq!(narrow.len(), 2);
        assert_eq!(row_text(&narrow[0]), "Lyrics from Unison");
        assert_eq!(row_text(&narrow[1]), "submitted by boidu");
    }
}
