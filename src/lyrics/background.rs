use super::{join_syllables, Lyrics, LyricLine, Syllable, SyncKind, VocalGroup};

const OPEN: [char; 2] = ['(', '（'];
const CLOSE: [char; 2] = [')', '）'];
const SENTENCE_END: [char; 7] = ['.', '!', '?', '…', '。', '！', '？'];
const SOFT_END: [char; 3] = [',', ';', ':'];
const TIGHT: [char; 7] = [',', '.', '!', '?', ';', ':', '…'];

#[derive(Default)]
struct Split {
    touched: bool,
    outside: String,
    inside: Vec<(usize, String)>,
}

fn split_parts<'a>(parts: impl Iterator<Item = &'a str>) -> Option<Vec<Split>> {
    let mut depth = 0usize;
    let mut region = 0usize;
    let mut regions = 0usize;
    let mut out = Vec::new();
    for text in parts {
        let mut split = Split {
            touched: depth > 0,
            ..Default::default()
        };
        for c in text.chars() {
            if OPEN.contains(&c) {
                depth += 1;
                split.touched = true;
                if depth == 1 {
                    region = regions;
                    regions += 1;
                    continue;
                }
            } else if CLOSE.contains(&c) {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    continue;
                }
            }
            if depth == 0 {
                split.outside.push(c);
            } else {
                match split.inside.last_mut() {
                    Some((r, s)) if *r == region => s.push(c),
                    _ => split.inside.push((region, c.to_string())),
                }
            }
        }
        out.push(split);
    }
    (depth == 0 && regions > 0).then_some(out)
}

fn tidy(s: &str) -> String {
    let mut out = String::new();
    for word in s.split_whitespace() {
        let word = if out.ends_with(SOFT_END) {
            word.trim_start_matches(SOFT_END)
        } else {
            word
        };
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() && !word.starts_with(TIGHT) {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

fn append_punct(target: &mut String, punct: &str) {
    let punct = if target.ends_with(SOFT_END) {
        punct.trim_start_matches(SOFT_END)
    } else {
        punct
    };
    target.push_str(punct);
}

fn strip_parens(s: &str) -> String {
    tidy(&s.replace(OPEN, " ").replace(CLOSE, " "))
}

fn capitalize(text: &mut String) -> bool {
    let Some((i, c)) = text.char_indices().find(|(_, c)| c.is_alphabetic()) else {
        return false;
    };
    let upper: String = c.to_uppercase().collect();
    text.replace_range(i..i + c.len_utf8(), &upper);
    true
}

fn capitalize_text(phrase: &mut [Syllable]) {
    for s in phrase.iter_mut() {
        if capitalize(&mut s.text) {
            break;
        }
    }
}

fn capitalize_roman(phrase: &mut [Syllable]) {
    for r in phrase.iter_mut().filter_map(|s| s.roman.as_mut()) {
        if capitalize(r) {
            break;
        }
    }
}

fn join_phrases(phrases: Vec<Vec<Syllable>>) -> Vec<Syllable> {
    let mut out: Vec<Syllable> = Vec::new();
    for mut phrase in phrases.into_iter().filter(|p| !p.is_empty()) {
        if let Some(prev) = out.last_mut() {
            if prev.text.ends_with(SENTENCE_END) {
                capitalize_text(&mut phrase);
            } else if !prev.text.ends_with(SOFT_END) {
                prev.text.push(',');
            }
            if let Some(r) = prev.roman.as_mut() {
                if r.ends_with(SENTENCE_END) {
                    capitalize_roman(&mut phrase);
                } else if !r.ends_with(SOFT_END) {
                    r.push(',');
                }
            }
        }
        if let Some(last) = phrase.last_mut() {
            last.part_of_word = false;
            last.roman_part_of_word = false;
        }
        out.extend(phrase);
    }
    capitalize_text(&mut out);
    capitalize_roman(&mut out);
    out
}

fn plain(text: String) -> Syllable {
    Syllable {
        text,
        start: 0.0,
        end: 0.0,
        part_of_word: false,
        roman: None,
        roman_part_of_word: false,
    }
}

fn split_text(text: &str) -> Option<(String, String)> {
    let split = split_parts(std::iter::once(text))?.pop()?;
    let mut phrases: Vec<Vec<Syllable>> = Vec::new();
    for (r, t) in split.inside {
        let t = tidy(&t);
        if t.is_empty() {
            continue;
        }
        if phrases.len() <= r {
            phrases.resize(r + 1, Vec::new());
        }
        phrases[r].push(plain(t));
    }
    let joined = join_phrases(phrases);
    let outside = tidy(&split.outside);
    let outside = outside.trim_matches(SOFT_END).trim().to_string();
    Some((outside, join_syllables(joined.iter().map(|s| (s.text.as_str(), false)))))
}

fn split_line(line: &mut LyricLine) {
    let syllables = &line.lead.syllables;
    let Some(splits) = split_parts(syllables.iter().map(|s| s.text.as_str())) else {
        return;
    };
    let romans = syllables
        .iter()
        .any(|s| s.roman.is_some())
        .then(|| split_parts(syllables.iter().map(|s| s.roman.as_deref().unwrap_or(&s.text))))
        .flatten()
        .filter(|r| r.len() == splits.len());
    let regions = splits
        .iter()
        .flat_map(|s| s.inside.iter().map(|(r, _)| r + 1))
        .max()
        .unwrap_or(0);
    let mut lead: Vec<Syllable> = Vec::new();
    let mut phrases: Vec<Vec<Syllable>> = vec![Vec::new(); regions];
    for (k, (syl, split)) in syllables.iter().zip(&splits).enumerate() {
        let roman_split = romans.as_ref().map(|r| &r[k]).filter(|_| syl.roman.is_some());
        for (r, text) in &split.inside {
            let text = tidy(text);
            if text.is_empty() {
                continue;
            }
            let roman = match roman_split {
                Some(rs) => rs.inside.iter().find(|(rr, _)| rr == r).map(|(_, t)| tidy(t)),
                None => syl.roman.as_deref().map(strip_parens),
            }
            .filter(|t| !t.is_empty());
            phrases[*r].push(Syllable {
                text,
                roman,
                ..syl.clone()
            });
        }
        if !split.touched {
            lead.push(syl.clone());
            continue;
        }
        let text = tidy(&split.outside);
        let roman = roman_split.map(|rs| tidy(&rs.outside)).filter(|t| !t.is_empty());
        if text.chars().any(char::is_alphanumeric) {
            lead.push(Syllable {
                text,
                roman,
                ..syl.clone()
            });
        } else if let Some(prev) = lead.last_mut() {
            prev.part_of_word &= syl.part_of_word;
            prev.roman_part_of_word &= syl.roman_part_of_word;
            append_punct(&mut prev.text, &text);
            if let (Some(pr), Some(r)) = (prev.roman.as_mut(), roman) {
                append_punct(pr, &r);
            }
        }
    }
    let background = join_phrases(phrases);
    let (Some(first), Some(last)) = (background.first(), background.last()) else {
        return;
    };
    let (start, end) = (first.start, last.end);
    if let Some(first) = lead.first_mut() {
        first.text = first.text.trim_start_matches(SOFT_END).trim_start().to_string();
    }
    if let Some(last) = lead.last_mut() {
        last.text = last.text.trim_end_matches(SOFT_END).trim_end().to_string();
        if let Some(r) = last.roman.as_mut() {
            *r = r.trim_end_matches(SOFT_END).trim_end().to_string();
        }
    }
    lead.retain(|s| !s.text.is_empty());
    let mut bg_roman = None;
    if let Some((outside, inside)) = line.lead.roman.as_deref().and_then(split_text) {
        line.lead.roman = (!outside.is_empty()).then_some(outside);
        bg_roman = (!inside.is_empty()).then_some(inside);
    }
    if let (Some(first), Some(last)) = (lead.first(), lead.last()) {
        line.lead.start = first.start;
        line.lead.end = last.end;
    }
    line.lead.syllables = lead;
    line.background = vec![VocalGroup {
        start,
        end,
        syllables: background,
        roman: bg_roman,
        translation: None,
    }];
}

pub fn split_background(lyrics: &mut Lyrics) {
    if lyrics.kind != SyncKind::Syllable {
        return;
    }
    let mut out: Vec<LyricLine> = Vec::with_capacity(lyrics.lines.len());
    for mut line in std::mem::take(&mut lyrics.lines) {
        let eligible = !line.background_only && line.background.iter().all(|g| g.syllables.is_empty());
        if eligible {
            split_line(&mut line);
        }
        let whole = eligible && line.lead.syllables.is_empty() && !line.background.is_empty();
        if let (true, Some(prev)) = (whole, out.last_mut()) {
            if let Some(g) = line.background.first_mut() {
                g.translation = g.translation.take().or(line.lead.translation.take());
            }
            prev.background.append(&mut line.background);
            continue;
        }
        out.push(line);
    }
    lyrics.lines = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::Credits;

    fn line(words: &[&str]) -> LyricLine {
        let syllables: Vec<Syllable> = words
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let (text, part_of_word) = match w.strip_suffix('~') {
                    Some(t) => (t, true),
                    None => (*w, false),
                };
                Syllable {
                    text: text.into(),
                    start: i as f64,
                    end: i as f64 + 1.0,
                    part_of_word,
                    roman: None,
                    roman_part_of_word: false,
                }
            })
            .collect();
        let end = syllables.len() as f64;
        LyricLine {
            start: 0.0,
            end,
            opposite: false,
            lead: VocalGroup {
                start: 0.0,
                end,
                syllables,
                roman: None,
                translation: None,
            },
            background: vec![],
            background_only: false,
        }
    }

    fn lyrics(kind: SyncKind, lines: Vec<LyricLine>) -> Lyrics {
        Lyrics {
            kind,
            lines,
            credits: Credits::default(),
            language: None,
            instrumental: false,
        }
    }

    fn fixed(words: &[&str]) -> LyricLine {
        let mut l = lyrics(SyncKind::Syllable, vec![line(words)]);
        split_background(&mut l);
        l.lines.remove(0)
    }

    #[test]
    fn moves_parenthesized_words_to_background() {
        let l = fixed(&["5", "in", "a", "row", "(yup),", "I", "want", "some", "mo'", "(yup)"]);
        assert_eq!(l.lead.text(), "5 in a row, I want some mo'");
        assert_eq!(l.background.len(), 1);
        let bg = &l.background[0];
        assert_eq!(bg.text(), "Yup, yup");
        assert_eq!(bg.syllables[0].start, 4.0);
        assert_eq!(bg.syllables[1].start, 9.0);
        assert_eq!((bg.start, bg.end), (4.0, 10.0));
        assert_eq!((l.lead.start, l.lead.end), (0.0, 9.0));
    }

    #[test]
    fn keeps_capitalized_words_and_capitalizes_the_start() {
        let l = fixed(&[
            "On", "my", "nose", "a", "ban~", "dan~", "na", "('dan~", "na),", "on", "my", "neck", "is", "a", "Fan~", "ta",
            "(Fan~", "ta)",
        ]);
        assert_eq!(l.lead.text(), "On my nose a bandanna, on my neck is a Fanta");
        assert_eq!(l.background[0].text(), "'Danna, Fanta");
        assert_eq!(l.background[0].syllables.len(), 4);
    }

    #[test]
    fn sentence_endings_skip_the_comma_and_capitalize() {
        let l = fixed(&["5", "in", "a", "row", "(yup!),", "I", "want", "some", "mo'", "(yup)"]);
        assert_eq!(l.lead.text(), "5 in a row, I want some mo'");
        assert_eq!(l.background[0].text(), "Yup! Yup");
    }

    #[test]
    fn splits_phrases_across_syllables() {
        let l = fixed(&["Cribs", "overseas", "(for", "cribs", "o~", "ver~", "seas)"]);
        assert_eq!(l.lead.text(), "Cribs overseas");
        assert_eq!(l.background[0].text(), "For cribs overseas");
        assert_eq!(l.background[0].syllables.len(), 5);
    }

    #[test]
    fn handles_lone_parentheses() {
        let l = fixed(&["Go", "(", "go)", "now"]);
        assert_eq!(l.lead.text(), "Go now");
        assert_eq!(l.background[0].text(), "Go");
        assert_eq!(l.background[0].syllables[0].start, 2.0);
    }

    #[test]
    fn drops_dangling_commas() {
        let l = fixed(&["(Yeah),", "I'm", "here,", "(yeah)"]);
        assert_eq!(l.lead.text(), "I'm here");
        assert_eq!(l.background[0].text(), "Yeah, yeah");
        let l = fixed(&["Hey,", "(yeah),", "you", "(you),"]);
        assert_eq!(l.lead.text(), "Hey, you");
        assert_eq!(l.background[0].text(), "Yeah, you");
    }

    fn shifted(words: &[&str], by: f64) -> LyricLine {
        let mut l = line(words);
        l.start += by;
        l.end += by;
        l.lead.start += by;
        l.lead.end += by;
        for s in &mut l.lead.syllables {
            s.start += by;
            s.end += by;
        }
        l
    }

    #[test]
    fn whole_line_attaches_to_the_previous_line() {
        let mut first = line(&["Hey", "you"]);
        first.background.push(VocalGroup::single("there".into(), 1.0, 2.0));
        let mut l = lyrics(
            SyncKind::Syllable,
            vec![first, shifted(&["(ooh,", "ooh)"], 2.0), shifted(&["(yeah)"], 4.0), shifted(&["Next"], 5.0)],
        );
        split_background(&mut l);
        l.sort();
        assert_eq!(l.lines.len(), 2);
        let bg = &l.lines[0].background;
        assert_eq!(bg.len(), 3);
        assert_eq!(bg[0].text(), "there");
        assert_eq!(bg[1].text(), "Ooh, ooh");
        assert_eq!((bg[1].start, bg[1].end), (2.0, 4.0));
        assert_eq!(bg[2].text(), "Yeah");
        assert_eq!(l.lines[0].full_end(), 5.0);
        assert_eq!(l.lines[1].lead.text(), "Next");
    }

    #[test]
    fn whole_first_line_becomes_background_only() {
        let mut l = lyrics(SyncKind::Syllable, vec![line(&["(Ooh,", "ooh)"]), shifted(&["Hey"], 2.0)]);
        split_background(&mut l);
        l.sort();
        assert_eq!(l.lines.len(), 2);
        assert!(l.lines[0].background_only);
        assert_eq!(l.lines[0].lead.text(), "Ooh, ooh");
    }

    #[test]
    fn leaves_lines_with_background_alone() {
        let mut original = line(&["Hey", "(you)"]);
        original.background.push(VocalGroup::single("there".into(), 2.0, 3.0));
        let mut l = lyrics(SyncKind::Syllable, vec![original]);
        split_background(&mut l);
        assert_eq!(l.lines[0].lead.text(), "Hey (you)");
        assert_eq!(l.lines[0].background[0].text(), "there");
    }

    #[test]
    fn leaves_other_sync_kinds_and_unbalanced_lines_alone() {
        let mut l = lyrics(SyncKind::Line, vec![line(&["Hey", "(you)"])]);
        split_background(&mut l);
        assert_eq!(l.lines[0].lead.text(), "Hey (you)");
        assert!(l.lines[0].background.is_empty());
        let l = fixed(&["(still", "going"]);
        assert_eq!(l.lead.text(), "(still going");
        assert!(l.background.is_empty());
    }

    #[test]
    fn splits_romanization() {
        let mut original = line(&["사랑", "(oh)"]);
        original.lead.syllables[0].roman = Some("sarang".into());
        original.lead.syllables[1].roman = Some("(oh)".into());
        let mut l = lyrics(SyncKind::Syllable, vec![original]);
        split_background(&mut l);
        assert_eq!(l.lines[0].lead.roman_text().as_deref(), Some("sarang"));
        assert_eq!(l.lines[0].background[0].roman_text().as_deref(), Some("Oh"));
    }
}
