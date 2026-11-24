use std::time::Instant;

use serde_json::{Value, json};

use crate::app::{App, LyricsState};
use crate::lyrics::process::{self, words};
use crate::lyrics::{Lyrics, SyncKind};
use crate::player::Status;

#[derive(Debug, Default, PartialEq)]
pub struct Position {
    pub line: usize,
    pub text: String,
    pub progress: f64,
    pub word: String,
    pub next: String,
    pub interlude: bool,
}

pub fn locate(lyrics: &Lyrics, t: f64, interlude_after: f64) -> Position {
    let mut pos = Position::default();
    if lyrics.kind == SyncKind::Static || lyrics.lines.is_empty() {
        return pos;
    }
    let Some(i) = lyrics.lines.iter().rposition(|l| l.start <= t) else {
        pos.next = lyrics.lines[0].lead.text();
        pos.interlude = lyrics.lines[0].start >= interlude_after;
        return pos;
    };
    let line = &lyrics.lines[i];
    let end = std::iter::once(&line.lead)
        .chain(line.background.iter())
        .filter_map(|g| g.syllables.last().map(|s| s.end.max(g.start)))
        .fold(line.start, f64::max);
    let next = lyrics.lines.get(i + 1);
    pos.line = i + 1;
    pos.next = next.map(|n| n.lead.text()).unwrap_or_default();
    let gap = next.map(|n| n.start - end).unwrap_or(f64::INFINITY);
    if t >= end && gap >= interlude_after {
        pos.interlude = next.is_some();
        pos.progress = 1.0;
        return pos;
    }
    pos.text = line.lead.text();
    pos.progress = process::progress(t, line.start, end);
    if lyrics.kind == SyncKind::Syllable {
        let spans = words(&line.lead);
        if let Some(w) = spans.iter().rev().find(|w| w.start <= t).or(spans.first()) {
            pos.word = line.lead.syllables[w.syllables.clone()]
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>()
                .trim()
                .to_string();
        }
    }
    pos
}

fn status(app: &App) -> &'static str {
    match &app.snapshot {
        None => "no player",
        Some(s) if s.track.is_none() => "stopped",
        Some(s) => match s.status {
            Status::Playing => "playing",
            Status::Paused => "paused",
            Status::Stopped => "stopped",
        },
    }
}

fn lyrics_state(app: &App) -> &'static str {
    match &app.lyrics {
        LyricsState::NoPlayer => "no player",
        LyricsState::NoTrack => "no track",
        LyricsState::Unsupported(_) => "unsupported",
        LyricsState::Waiting | LyricsState::Loading => "loading",
        LyricsState::Ready { .. } => "ready",
        LyricsState::NotFound(_) => "not found",
    }
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

pub fn snapshot(app: &App, now: Instant) -> Value {
    let track = app.track.as_ref();
    let position = if app.snapshot.is_some() { app.position(now).max(0.0) } else { 0.0 };
    let offset = (app.cfg.sync.offset_ms + app.track_offset_ms()) as f64 / 1000.0;
    let mut v = json!({
        "status": status(app),
        "player": app.snapshot.as_ref().map(|s| s.player.display_name()).unwrap_or_default(),
        "title": track.map(|t| t.title.clone()).unwrap_or_default(),
        "artist": track.map(|t| t.query().artist_line()).unwrap_or_default(),
        "artists": track.map(|t| t.query().artists).unwrap_or_default(),
        "album": track.and_then(|t| t.album.clone()).unwrap_or_default(),
        "position": round(position),
        "length": round(track.and_then(|t| t.length).unwrap_or(0.0)),
        "lyrics": lyrics_state(app),
        "sync": "",
        "provider": "",
        "source": "",
        "line": 0,
        "lines": 0,
        "text": "",
        "progress": 0.0,
        "word": "",
        "next": "",
        "interlude": false,
        "offset": offset,
    });
    if let Some(t) = track {
        let q = t.query();
        v["spotifyId"] = json!(q.spotify_id.unwrap_or_default());
        v["isrc"] = json!(q.isrc.unwrap_or_default());
    }
    if let LyricsState::Ready { resolution, prepared, .. } = &app.lyrics {
        let lyrics = &prepared.lyrics;
        let threshold = app.cfg.lyrics.interlude_threshold_secs;
        let at = locate(lyrics, position + offset, threshold);
        v["sync"] = json!(match lyrics.kind {
            SyncKind::Syllable => "word",
            SyncKind::Line => "line",
            SyncKind::Static => "unsynced",
        });
        v["provider"] = json!(crate::providers::provider_name(&resolution.candidate.provider));
        v["source"] = json!(lyrics.credits.source.clone().unwrap_or_default());
        v["line"] = json!(at.line);
        v["lines"] = json!(lyrics.lines.len());
        v["text"] = json!(at.text);
        v["progress"] = json!(round(at.progress));
        v["word"] = json!(at.word);
        v["next"] = json!(at.next);
        v["interlude"] = json!(at.interlude);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_line_and_word_being_sung() {
        let (lyrics, _) = crate::library::parse(
            "[00:01.00]<00:01.00>first <00:01.50>line<00:02.00>\n[00:02.00]<00:02.00>second<00:03.00>\n[00:10.00]<00:10.00>after<00:11.00>\n",
            &crate::library::Source::Clipboard,
            None,
            Some(20.0),
        )
        .unwrap();
        assert_eq!(lyrics.kind, SyncKind::Syllable);
        let before = locate(&lyrics, 0.5, 3.0);
        assert_eq!((before.line, before.next.as_str()), (0, "first line"));
        let at = locate(&lyrics, 1.6, 3.0);
        assert_eq!((at.line, at.text.as_str(), at.word.as_str()), (1, "first line", "line"));
        assert!(at.progress > 0.5 && at.progress < 1.0);
        let gap = locate(&lyrics, 5.0, 3.0);
        assert_eq!((gap.line, gap.text.as_str(), gap.interlude), (2, "", true));
        assert_eq!(gap.next, "after");
        let short = locate(&lyrics, 5.0, 30.0);
        assert_eq!(short.text, "second");
    }
}
