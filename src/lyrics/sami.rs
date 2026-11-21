use super::cue::{self, Cue};
use super::{Credits, Lyrics};

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        if at > 0 && lower.as_bytes()[at - 1].is_ascii_alphanumeric() {
            continue;
        }
        let rest = lower[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let offset = tag.len() - rest.len();
        let raw = tag[offset..].trim_start();
        let value: String = if let Some(q) = raw.strip_prefix(['"', '\'']) {
            q.chars().take_while(|c| *c != '"' && *c != '\'').collect()
        } else {
            raw.chars().take_while(|c| !c.is_whitespace() && *c != '>').collect()
        };
        return Some(value);
    }
    None
}

fn paragraphs(content: &str) -> Vec<(Option<String>, String)> {
    let lower = content.to_ascii_lowercase();
    let starts: Vec<usize> = lower
        .match_indices("<p")
        .map(|(i, _)| i)
        .filter(|i| lower[i + 2..].starts_with(['>', ' ', '\t', '\n', '\r']))
        .collect();
    if starts.is_empty() {
        return vec![(None, content.to_string())];
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &i)| {
            let end = starts.get(n + 1).copied().unwrap_or(content.len());
            let tag_end = content[i..end].find('>').map(|t| i + t + 1).unwrap_or(end);
            (attr_value(&content[i..tag_end], "class"), content[tag_end..end].to_string())
        })
        .collect()
}

pub fn cues(input: &str) -> Vec<Cue> {
    let lower = input.to_ascii_lowercase();
    let body_end = lower.rfind("</body").unwrap_or(input.len());
    let syncs: Vec<usize> = lower.match_indices("<sync").map(|(i, _)| i).filter(|i| *i < body_end).collect();
    let mut points: Vec<(f64, String)> = Vec::new();
    let mut class: Option<Option<String>> = None;
    for (n, &i) in syncs.iter().enumerate() {
        let Some(tag_len) = input[i..].find('>') else { continue };
        let Some(start) = attr_value(&input[i..i + tag_len], "start").and_then(|v| v.parse::<f64>().ok()) else {
            continue;
        };
        let end = syncs.get(n + 1).copied().unwrap_or(body_end);
        let content = &input[i + tag_len + 1..end.max(i + tag_len + 1)];
        let paras = paragraphs(content);
        let chosen = class.get_or_insert_with(|| paras.first().and_then(|(c, _)| c.clone())).clone();
        let text = paras
            .iter()
            .find(|(c, _)| chosen.is_none() || *c == chosen)
            .map(|(_, t)| cue::squash(&cue::strip_markup(t)))
            .unwrap_or_default();
        points.push((start / 1000.0, text));
    }
    points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::new();
    for (i, (start, text)) in points.iter().enumerate() {
        if text.is_empty() {
            continue;
        }
        let end = points.get(i + 1).map(|(t, _)| *t).unwrap_or(start + 5.0);
        out.push(Cue {
            start: *start,
            end,
            text: text.clone(),
            segments: Vec::new(),
            voice: None,
        });
    }
    out
}

pub fn parse(input: &str, credits: Credits) -> Option<Lyrics> {
    cue::build(cues(input), credits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sami() {
        let src = "<SAMI><HEAD><STYLE>.ENCC {Name: English;}</STYLE></HEAD><BODY>\n<SYNC Start=1000><P Class=ENCC>Hello<br>there\n<SYNC Start=2500><P Class=ENCC>&nbsp;\n<SYNC Start=3000><P Class=ENCC>Bye &amp; go<P Class=FRCC>Salut\n</BODY></SAMI>";
        let l = parse(src, Credits::default()).unwrap();
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].lead.text(), "Hello there");
        assert!((l.lines[0].end - 2.5).abs() < 1e-9);
        assert_eq!(l.lines[1].lead.text(), "Bye & go");
    }
}
